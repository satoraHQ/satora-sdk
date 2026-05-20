//! `Client::fund_swap_gasless` — the user-facing entry point for the
//! ERC-4337 + EIP-7702 gasless funding flow.
//!
//! Sits in its own file (rather than in `client.rs`) so the
//! feature-gated alloy machinery doesn't leak into the base SDK's
//! source. Rust inherent impls can span files within a crate, so the
//! method lands on [`Client`] as if it were declared in `client.rs`.

use crate::Client;
use crate::aa::abi::Call;
use crate::aa::abi::IERC20;
use crate::aa::bundler::BundlerCasing;
use crate::aa::bundler::BundlerClient;
use crate::aa::orchestrate::FundSwapClients;
use crate::aa::orchestrate::FundSwapInputs;
use crate::aa::orchestrate::FundSwapReceipt;
use crate::aa::orchestrate::PaymasterRef;
use crate::aa::orchestrate::fund_swap;
use crate::aa::paymaster::PaymasterClient;
use crate::error::Error;
use crate::error::Result;
use alloy::primitives::Address;
use alloy::primitives::B256;
use alloy::primitives::U256;
use alloy::providers::Provider;
use alloy::providers::RootProvider;
use alloy::sol_types::SolCall;
use serde::Deserialize;
use serde_json::Value;
use std::str::FromStr;
use url::Url;

/// Bundler + node-RPC + optional paymaster configuration the gasless
/// flow needs.
///
/// `paymaster` is `None` when there's no sponsor — alto-against-Anvil
/// dev setups, for instance, where the depositor EOA pays its own gas
/// (EntryPoint pulls reimbursement from the sender, not a third party).
/// When `Some`, the SDK runs the ERC-7677 `pm_*` dance and embeds the
/// resulting `paymasterAndData` so a real paymaster picks up the bill.
#[derive(Debug, Clone)]
pub struct AaConfig {
    pub bundler_url: Url,
    pub node_rpc_url: Url,
    pub paymaster: Option<PaymasterConfig>,
    pub bundler_casing: BundlerCasing,
    /// When `Some`, skip the bundler's `eth_estimateUserOperationGas`
    /// call entirely and use these values directly. The intended use
    /// is to work around bundlers (alto in particular) that
    /// intermittently mis-simulate the userOp — see the long
    /// debugging story in commits `eeedef55` / `8df0bf5e` / the
    /// feedback memo on alto's estimation flake.
    ///
    /// Pass values with headroom over the SDK's observed peaks (e.g.
    /// `call_gas_limit ~ 500_000`, `verification_gas_limit ~ 150_000`,
    /// `pre_verification_gas ~ 100_000` for a USDC→tBTC swap on
    /// Arbitrum). Underestimating any of these reverts the userOp
    /// on-chain — overestimating just over-prefunds and refunds the
    /// excess.
    pub gas_overrides: Option<GasOverrides>,
}

/// Explicit gas limits that bypass the bundler's gas-estimation RPC.
/// Mirrors the three fields `eth_estimateUserOperationGas` normally
/// returns; using them inline skips the call entirely.
///
/// All three values are `u64` — typical userOp gas limits sit in the
/// 100k–500k range, well inside `u64`. The SDK widens them to
/// `U256` before assigning to the userOp.
#[derive(Debug, Clone, Copy)]
pub struct GasOverrides {
    pub call_gas_limit: u64,
    pub verification_gas_limit: u64,
    pub pre_verification_gas: u64,
}

/// Optional paymaster sponsorship — URL + context as a unit, since one
/// without the other doesn't address any real paymaster.
#[derive(Debug, Clone)]
pub struct PaymasterConfig {
    /// Paymaster RPC URL. Often identical to the bundler URL (Alchemy
    /// co-locates them).
    pub url: Url,
    /// Paymaster-specific context object. For Alchemy Gas Manager:
    /// `serde_json::json!({"policyId": "<uuid>"})`. Use `Value::Null`
    /// for paymasters that don't take one.
    pub context: Value,
}

impl Client {
    /// `GET /aa/config?chain={id}` — fetch the server-managed bundler
    /// (and optional paymaster) details for an EVM chain.
    ///
    /// Lets consumers avoid hard-coding bundler URLs (which embed a
    /// provider API key the server rotates). Use
    /// [`RemoteAaConfig::into_config`] to combine the response with the
    /// caller-supplied node RPC, paymaster context, and gas overrides
    /// into the full [`AaConfig`] [`Self::fund_swap_gasless`] expects.
    ///
    /// Returns [`Error::Api`] with HTTP 404 when the server has no
    /// bundler configured for `chain`, or HTTP 400 for non-EVM chains.
    pub async fn fetch_aa_config(
        &self,
        chain: crate::types::Chain,
    ) -> Result<crate::aa::remote_config::RemoteAaConfig> {
        self.send(crate::aa::remote_config::AaConfigRequest::new(chain))
            .await
    }

    /// Submit a gasless EVM funding UserOp for `swap_id`.
    ///
    /// Looks up the per-swap `key_index` from storage, derives the
    /// ephemeral EOA via the configured [`crate::Signer`], fetches the
    /// backend's `/swap/{id}/swap-and-lock-calldata-userop` payload,
    /// then drives the full bundler + paymaster + 7702 flow via
    /// [`fund_swap`].
    ///
    /// Errors:
    /// - [`Error::InvalidSigner`] if the client wasn't built with a signer.
    /// - [`Error::InvalidSwap`] if `swap_id` isn't in storage (no `key_index`).
    /// - [`Error::Transport`] / [`Error::Api`] for backend, bundler, paymaster, or node RPC
    ///   failures.
    pub async fn fund_swap_gasless(
        &self,
        swap_id: &str,
        aa_config: AaConfig,
    ) -> Result<FundSwapReceipt> {
        // 1. EOA derivation from the per-swap key_index.
        let signer = self.signer.as_ref().ok_or_else(|| {
            Error::InvalidSigner(
                "Client constructed without a signer — use Client::builder() with .mnemonic / .xprv"
                    .to_string(),
            )
        })?;
        let key_index = self.storage.get_swap_key_index(swap_id)?.ok_or_else(|| {
            Error::InvalidSwap(format!(
                "no key_index in storage for swap `{swap_id}` — has create_swap been called on this Client?",
            ))
        })?;
        let evm_key = signer.derive_evm_key(key_index)?;
        let eoa_address = parse_address(&evm_key.address, "eoa")?;

        // 2. Backend payload fetch.
        let backend = self.fetch_userop_calldata(swap_id).await?;

        // 3. RPC clients. Capture `gas_overrides` first because the field-by-field moves of
        //    `aa_config` below would consume the value (Option<GasOverrides> is Copy via its inner
        //    Copy derive, so this is cheap).
        let gas_overrides = aa_config.gas_overrides;
        let bundler = BundlerClient::new(aa_config.bundler_url, aa_config.bundler_casing)?;
        let paymaster_client = aa_config
            .paymaster
            .as_ref()
            .map(|pm| PaymasterClient::new(pm.url.clone(), aa_config.bundler_casing))
            .transpose()?;
        let node = RootProvider::new_http(aa_config.node_rpc_url);
        let chain_id = node
            .get_chain_id()
            .await
            .map_err(|e| Error::Transport(format!("eth_chainId: {e}")))?;

        // 4. Map backend wire types onto the orchestration input.
        let inputs = build_fund_swap_inputs(
            backend,
            evm_key.secret_key,
            eoa_address,
            chain_id,
            gas_overrides,
        )?;

        // 5. Drive the orchestration. The PaymasterRef borrows both the client and the
        //    user-supplied context — bundled so the orchestrate layer can't see one without the
        //    other.
        let paymaster_ref = paymaster_client.as_ref().map(|client| PaymasterRef {
            client,
            context: aa_config
                .paymaster
                .as_ref()
                .map(|pm| pm.context.clone())
                .unwrap_or(Value::Null),
        });
        fund_swap(
            inputs,
            FundSwapClients {
                bundler: &bundler,
                node: &node,
                paymaster: paymaster_ref,
            },
        )
        .await
    }

    /// Poll until the gasless deposit address holds enough source token
    /// AND enough native gas. Real users send funds to the address out-
    /// of-band (wallet, exchange, hardware device); this helper lets a
    /// caller wait on that arrival before invoking
    /// [`Self::fund_swap_gasless`].
    ///
    /// Resolves the deposit address + required token amount from the
    /// swap response itself; the caller only supplies the gas headroom
    /// they want to require (in wei). Without a paymaster the SDK
    /// recommends ~0.001 ETH headroom for a typical USDC→tBTC userOp;
    /// with a paymaster, pass 0.
    ///
    /// Returns `Error::Timeout` if `timeout` elapses before both
    /// thresholds are met. 5s poll interval, fixed.
    #[tracing::instrument(name = "wait_for_deposit_funding", skip_all, fields(%swap_id, min_eth_wei, ?timeout))]
    pub async fn wait_for_deposit_funding(
        &self,
        swap_id: &str,
        aa_config: &AaConfig,
        min_eth_wei: u64,
        timeout: std::time::Duration,
    ) -> Result<()> {
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

        let resp = self.fetch_swap_response(swap_id).await?;
        let deposit_address = parse_address(&resp.client_evm_address, "client_evm_address")?;
        let token_address = parse_address(
            resp.source_token.token_id.as_wire_str(),
            "source_token_address",
        )?;
        let min_token_units = parse_u256_dec(&resp.source_amount, "source_amount")?;
        let min_eth = U256::from(min_eth_wei);

        // Explicit `RootProvider` (defaults to Ethereum network) — the
        // generic param is otherwise ambiguous when only `Provider`
        // methods (no Network-specific filler) are touched.
        let node: RootProvider = RootProvider::new_http(aa_config.node_rpc_url.clone());
        // The IERC20 ABI's `balanceOf(address)` selector + calldata.
        // Encoded once per loop iteration since the deposit address
        // doesn't change.
        let balance_calldata = IERC20::balanceOfCall {
            account: deposit_address,
        }
        .abi_encode();
        let balance_call = alloy::rpc::types::TransactionRequest::default()
            .to(token_address)
            .input(balance_calldata.into());

        let deadline = std::time::Instant::now() + timeout;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let eth_bal = node
                .get_balance(deposit_address)
                .await
                .map_err(|e| Error::Transport(format!("eth_getBalance: {e}")))?;
            let token_bal_bytes = node
                .call(balance_call.clone())
                .await
                .map_err(|e| Error::Transport(format!("balanceOf eth_call: {e}")))?;
            // Solidity returns uint256 as 32 BE bytes; `U256::from_be_slice`
            // accepts that directly.
            let token_bal = U256::from_be_slice(token_bal_bytes.as_ref());
            tracing::debug!(
                attempt,
                eth_wei = %eth_bal,
                token_units = %token_bal,
                "wait_for_deposit_funding: poll",
            );
            if token_bal >= min_token_units && eth_bal >= min_eth {
                tracing::info!(
                    attempt,
                    eth_wei = %eth_bal,
                    token_units = %token_bal,
                    "wait_for_deposit_funding: thresholds met",
                );
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "deposit address {deposit_address:#x} did not receive \
                     enough funding within {timeout:?} (have: {token_bal} token units / {eth_bal} wei; \
                     need: {min_token_units} / {min_eth})",
                )));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    async fn fetch_userop_calldata(&self, swap_id: &str) -> Result<UseropFundingCalldataResponse> {
        // GET /swap/{id}/swap-and-lock-calldata-userop — sidesteps the
        // `Endpoint` trait because the trait's `PATH` is static, and
        // this URL is per-swap.
        let url = self
            .base_url
            .join(&format!("/swap/{swap_id}/swap-and-lock-calldata-userop"))?;
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(Error::Api {
                status: status.as_u16(),
                message: body,
            });
        }
        // Read the body as text first so a decode error can include the
        // raw JSON in the message — `resp.json::<T>()` swallows that
        // context and you end up debugging blind.
        serde_json::from_str::<UseropFundingCalldataResponse>(&body)
            .map_err(|e| Error::Decode(format!("userop calldata response: {e} (body: {body})")))
    }
}

// ── backend response wire types ────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct UseropFundingCalldataResponse {
    coordinator_address: String,
    #[allow(dead_code)] // surfaced for callers but not consumed by orchestrate.
    permit2_address: String,
    source_token_address: String,
    /// Decimal string — the backend serialises u64 amounts via
    /// `serde_string` so they round-trip cleanly through JS clients
    /// that can't represent the full uint64 range as a Number.
    source_amount: String,
    lock_token_address: String,
    preimage_hash: String,
    claim_address: String,
    timelock: u64,
    calls: Vec<BackendCallJson>,
    calls_hash: String,
    #[allow(dead_code)]
    relay_fee: Option<String>,
    aa: BackendAaConfig,
}

#[derive(Debug, Clone, Deserialize)]
struct BackendCallJson {
    target: String,
    value: String,
    call_data: String,
}

#[derive(Debug, Clone, Deserialize)]
struct BackendAaConfig {
    entry_point: String,
    delegation_target: String,
}

fn build_fund_swap_inputs(
    backend: UseropFundingCalldataResponse,
    secret_key: [u8; 32],
    eoa_address: Address,
    chain_id: u64,
    gas_overrides: Option<GasOverrides>,
) -> Result<FundSwapInputs> {
    let calls = backend
        .calls
        .iter()
        .enumerate()
        .map(|(i, c)| {
            Ok(Call {
                target: parse_address(&c.target, &format!("calls[{i}].target"))?,
                value: parse_u256_dec(&c.value, &format!("calls[{i}].value"))?,
                callData: parse_bytes(&c.call_data, &format!("calls[{i}].call_data"))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(FundSwapInputs {
        coordinator_address: parse_address(&backend.coordinator_address, "coordinator_address")?,
        source_token_address: parse_address(&backend.source_token_address, "source_token_address")?,
        source_amount: parse_u256_dec(&backend.source_amount, "source_amount")?,
        lock_token_address: parse_address(&backend.lock_token_address, "lock_token_address")?,
        preimage_hash: parse_b256(&backend.preimage_hash, "preimage_hash")?,
        claim_address: parse_address(&backend.claim_address, "claim_address")?,
        timelock: backend.timelock,
        calls,
        calls_hash: parse_b256(&backend.calls_hash, "calls_hash")?,
        entry_point: parse_address(&backend.aa.entry_point, "aa.entry_point")?,
        delegation_target: parse_address(&backend.aa.delegation_target, "aa.delegation_target")?,
        secret_key,
        eoa_address,
        chain_id,
        gas_overrides,
    })
}

// ── small parse helpers — each wraps a primitive's parse error in our
//    `Error::Decode` so callers see a single failure shape.

fn parse_address(s: &str, field: &str) -> Result<Address> {
    Address::from_str(s).map_err(|e| Error::Decode(format!("{field} ({s}): {e}")))
}

fn parse_b256(s: &str, field: &str) -> Result<B256> {
    B256::from_str(s).map_err(|e| Error::Decode(format!("{field} ({s}): {e}")))
}

fn parse_u256_dec(s: &str, field: &str) -> Result<U256> {
    U256::from_str_radix(s, 10).map_err(|e| Error::Decode(format!("{field} ({s}): {e}")))
}

fn parse_bytes(s: &str, field: &str) -> Result<alloy::primitives::Bytes> {
    alloy::primitives::Bytes::from_str(s).map_err(|e| Error::Decode(format!("{field} ({s}): {e}")))
}
