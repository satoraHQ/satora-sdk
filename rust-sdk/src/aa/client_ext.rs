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
use crate::aa::orchestrate::fund_swap_observed;
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

/// Covers the configuration fetches, the pre-submission RPC calls and the bounded
/// receipt poll; the client-wide EVM lock is held for the whole call.
const GASLESS_FUNDING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

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

/// Caller-supplied knobs for [`Client::fund_swap_gasless`]. The bundler
/// URL, paymaster URL, EntryPoint, delegation target, and Permit2
/// address all come from the backend — those don't appear here. What
/// the SDK can't infer:
/// - which EVM node to talk to (depends on the consumer's provider stack);
/// - paymaster context (depends on the paymaster's policy ID etc);
/// - whether to bypass `eth_estimateUserOperationGas` and with what limits.
#[derive(Debug, Clone, Default)]
pub struct GaslessOpts {
    /// EVM node RPC URL the SDK uses for `eth_chainId`, balance reads,
    /// etc. `None` means "pick the chain's default" — the fund function
    /// resolves it via
    /// [`KnownChain::default_node_rpc_url`](crate::types::KnownChain::default_node_rpc_url)
    /// for the swap's deposit chain. Override with a private provider
    /// (Alchemy / Infura / a self-hosted node) for production volume.
    pub node_rpc_url: Option<Url>,
    /// Paymaster context (e.g. `{"policyId":"<uuid>"}` for Alchemy Gas
    /// Manager). Ignored when the backend returns no paymaster.
    /// `None` => `Value::Null`.
    pub paymaster_context: Option<Value>,
    /// Skip `eth_estimateUserOperationGas` with these limits — see
    /// [`GasOverrides`] for when this matters.
    pub gas_overrides: Option<GasOverrides>,
}

impl GaslessOpts {
    /// All-default opts — the fund function picks the node RPC URL
    /// from the swap's deposit chain. Equivalent to
    /// `GaslessOpts::default()`; named for discoverability.
    pub fn defaults() -> Self {
        Self::default()
    }

    /// Construct with an explicit node RPC URL. Convenience for the
    /// common "override the default" case.
    pub fn new(node_rpc_url: Url) -> Self {
        Self {
            node_rpc_url: Some(node_rpc_url),
            ..Self::default()
        }
    }

    pub fn with_paymaster_context(mut self, ctx: Value) -> Self {
        self.paymaster_context = Some(ctx);
        self
    }

    pub fn with_gas_overrides(mut self, overrides: GasOverrides) -> Self {
        self.gas_overrides = Some(overrides);
        self
    }
}

/// Single-shot snapshot of the depositor EOA's funding state for a
/// swap. Returned by [`Client::check_deposit_funding`].
///
/// Use `has_sufficient_source_token` to decide whether
/// [`Client::fund_swap_gasless`] is ready to call. Native balance is
/// reported separately so callers without a paymaster can also check
/// gas headroom before submitting.
#[derive(Debug, Clone)]
pub struct DepositStatus {
    /// Current source-token balance at the depositor EOA, in the
    /// smallest unit of the source token.
    pub source_token_balance: U256,
    /// Amount the depositor EOA must hold before the funding userOp
    /// can be submitted — same shape, mirrored from the swap response
    /// for convenient comparison.
    pub source_token_required: U256,
    /// Native (ETH / MATIC / …) balance at the depositor EOA, in wei.
    /// Matters only when the backend has no paymaster.
    pub native_balance_wei: U256,
    /// Convenience: `source_token_balance >= source_token_required`.
    pub has_sufficient_source_token: bool,
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
    /// [`crate::aa::RemoteAaConfig::into_config`] to combine the response with the
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
    /// Bundler and paymaster URLs come from the backend via
    /// [`Self::fetch_aa_config`]. The EntryPoint, Kernel delegation target
    /// and Permit2 addresses it reports must equal the pinned deployments.
    /// The caller only supplies the bits the server can't ship (node RPC,
    /// paymaster context, gas overrides) via [`GaslessOpts`]. The
    /// source chain is derived from the swap's `deposit_token`.
    ///
    /// Internally:
    /// 1. `GET /swap/{id}` to learn the source chain.
    /// 2. `GET /aa/config?chain={id}` for bundler/paymaster details.
    /// 3. Existing bundler + paymaster + 7702 flow via `fund_swap_observed`, which persists the
    ///    reservation and the operation hash.
    ///
    /// Errors:
    /// - [`Error::InvalidSigner`] if the client wasn't built with a signer.
    /// - [`Error::InvalidSwap`] if `swap_id` isn't in storage (no `key_index`), if a funding was
    ///   already submitted or is unresolved, if the swap's `deposit_token` doesn't map to a known
    ///   EVM chain, if it locks native currency or a hash lock not derived from the seed, if the
    ///   backend names an unpinned EntryPoint, delegation target or Permit2, or if its coordinator
    ///   differs from the one persisted for the swap.
    /// - [`Error::Timeout`] if the whole flow outlives its deadline; inspect the persisted
    ///   operation before retrying.
    /// - [`Error::Transport`] / [`Error::Api`] for backend, bundler, paymaster, or node RPC
    ///   failures.
    pub async fn fund_swap_gasless(
        &self,
        swap_id: &str,
        opts: GaslessOpts,
    ) -> Result<FundSwapReceipt> {
        tokio::time::timeout(
            GASLESS_FUNDING_TIMEOUT,
            self.fund_swap_gasless_inner(swap_id, opts),
        )
        .await
        .map_err(|_| {
            Error::Timeout(
                "gasless funding timed out; inspect the persisted operation before retrying".into(),
            )
        })?
    }

    async fn fund_swap_gasless_inner(
        &self,
        swap_id: &str,
        opts: GaslessOpts,
    ) -> Result<FundSwapReceipt> {
        let _execution = self.evm_execution.lock().await;
        let GaslessOpts {
            node_rpc_url,
            paymaster_context,
            gas_overrides,
        } = opts;
        let stored = self
            .get_stored_swap(swap_id)?
            .ok_or_else(|| Error::InvalidSwap("recover the swap before funding".into()))?;
        if stored.metadata.evm_fund_txid.is_some()
            || stored.metadata.evm_fund_userop_hash.is_some()
            || stored
                .metadata
                .bridge_operations
                .as_ref()
                .is_some_and(|operations| operations.contains_key("cctp_fund"))
        {
            return Err(Error::InvalidSwap(
                "funding already submitted; inspect its receipt before another attempt".into(),
            ));
        }
        if stored.metadata.relay_signature_outstanding() {
            return Err(Error::InvalidSwap(
                crate::evm::funding::UNRESOLVED_RELAY_FUNDING.into(),
            ));
        }
        if stored.metadata.evm_fund_started == Some(true) {
            return Err(Error::InvalidSwap(
                crate::evm::funding::UNRESOLVED_FUNDING.into(),
            ));
        }
        let response = self.get_swap_details(swap_id).await?;
        let context = crate::evm::EvmContext::from_response(&response)?;
        if !context.is_source {
            return Err(Error::InvalidSwap(
                "gasless funding requires an EVM input".into(),
            ));
        }
        if context.kind == crate::types::EvmHtlcKind::Native {
            return Err(Error::InvalidSwap(
                "funding a native lock is not supported by this SDK yet".into(),
            ));
        }
        crate::evm::verify_derived_hash_lock(self, &stored, &context)?;
        let (_, _, quoted_amount) = evm_deposit_fields(response)?;
        let chain = crate::types::KnownChain::from_wire_str(&context.chain_id.to_string());
        let remote = self
            .fetch_aa_config(crate::types::Chain::from_wire_str(
                &context.chain_id.to_string(),
            ))
            .await?;
        // The account delegates its code to this target and the EntryPoint executes
        // its calls, so neither may come from the API alone.
        if remote.entry_point != crate::aa::userop::ENTRY_POINT_V07
            || remote.delegation_target != crate::aa::kernel::KERNEL_DELEGATION_TARGET
        {
            return Err(Error::InvalidSwap(
                "server-named EntryPoint or delegation target is not the pinned deployment".into(),
            ));
        }
        let expected_entry_point = remote.entry_point;
        let expected_delegation_target = remote.delegation_target;
        if remote.permit2 != crate::aa::permit2::PERMIT2_ADDRESS {
            return Err(Error::InvalidSwap("unexpected Permit2 deployment".into()));
        }
        // Resolve the node RPC URL: explicit override wins; otherwise
        // use the per-chain default. Error if the chain has no default
        // and the caller didn't supply one (shouldn't happen for the
        // 3 EVM chains we support, but stays defensive against future
        // chains landing without a default URL set).
        let node_rpc_url = match node_rpc_url {
            Some(url) => url,
            None => {
                let default = chain.as_ref().and_then(|chain| chain.default_node_rpc_url()).ok_or_else(|| {
                    Error::InvalidSwap(format!(
                        "swap `{swap_id}` deposit chain `{}` has no default node RPC URL — pass GaslessOpts::new(url) with one explicitly",
                        context.chain_id,
                    ))
                })?;
                Url::parse(default).map_err(|e| {
                    Error::InvalidSwap(format!(
                        "internal: default node RPC URL `{default}` for chain `{}` failed to parse: {e}",
                        context.chain_id,
                    ))
                })?
            }
        };
        let aa_config = remote.into_config(
            node_rpc_url,
            paymaster_context.unwrap_or(Value::Null),
            gas_overrides,
        );

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
        let evm_key = signer.evm_key_for_address(key_index, &context.owner.to_string())?;
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
        let node = crate::evm::http_provider(aa_config.node_rpc_url);
        let chain_id = node
            .get_chain_id()
            .await
            .map_err(|e| Error::Transport(format!("eth_chainId: {e}")))?;

        if chain_id != context.chain_id {
            return Err(Error::InvalidSwap(
                "RPC chain ID does not match swap".into(),
            ));
        }
        if parse_address(&backend.permit2_address, "permit2")?
            != crate::aa::permit2::PERMIT2_ADDRESS
            || parse_address(&backend.aa.entry_point, "entry_point")? != expected_entry_point
            || parse_address(&backend.aa.delegation_target, "delegation_target")?
                != expected_delegation_target
            || parse_b256(&backend.preimage_hash, "preimage_hash")? != context.hash
            || parse_address(&backend.source_token_address, "source_token")? != context.source_token
            || parse_address(&backend.claim_address, "claim_address")? != context.server
            || backend.timelock != context.timelock
            // Both amounts come from the server, so this catches an inconsistent
            // payload, not a hostile one. A bridged deposit is shrunk by the bridge
            // fee, hence an upper bound rather than equality.
            || parse_u256_dec(&backend.source_amount, "source_amount")?
                > parse_u256_dec(&quoted_amount, "quoted source_amount")?
        {
            return Err(Error::InvalidSwap(
                "gasless funding payload does not match swap/configuration".into(),
            ));
        }
        let coordinator = backend.coordinator_address.clone();
        if stored
            .metadata
            .evm_coordinator_address
            .as_ref()
            .is_some_and(|pinned| !pinned.eq_ignore_ascii_case(&coordinator))
        {
            return Err(Error::InvalidSwap(
                "coordinator differs from the one persisted for this swap".into(),
            ));
        }
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
        let receipt = fund_swap_observed(
            inputs,
            FundSwapClients {
                bundler: &bundler,
                node: &node,
                paymaster: paymaster_ref,
            },
            &|| {
                self.update_swap_metadata(
                    swap_id,
                    crate::SwapMetadata {
                        evm_fund_started: Some(true),
                        evm_coordinator_address: Some(coordinator.clone()),
                        ..Default::default()
                    },
                )
            },
            &|hash| {
                self.update_swap_metadata(
                    swap_id,
                    crate::SwapMetadata {
                        evm_fund_userop_hash: Some(hash.to_string()),
                        evm_coordinator_address: Some(coordinator.clone()),
                        ..Default::default()
                    },
                )
            },
        )
        .await?;
        if let Some(hash) = receipt.transaction_hash {
            self.update_swap_metadata(
                swap_id,
                crate::SwapMetadata {
                    evm_fund_txid: Some(hash.to_string()),
                    evm_coordinator_address: Some(coordinator),
                    ..Default::default()
                },
            )?;
        }
        Ok(receipt)
    }

    /// Single-shot read of the depositor EOA's funding state — no
    /// polling, no waiting, no throw on "not funded yet". Use this for
    /// "is FundSwap ready?" branching; for the polling/waiting flow
    /// use [`Self::wait_for_deposit_funding`].
    ///
    /// Resolves the node RPC URL from the swap's deposit chain via
    /// [`crate::types::KnownChain::default_node_rpc_url`] when
    /// `node_rpc_url` is `None` — same per-chain defaults as
    /// [`Self::fund_swap_gasless`].
    #[tracing::instrument(name = "check_deposit_funding", skip_all, fields(%swap_id))]
    pub async fn check_deposit_funding(
        &self,
        swap_id: &str,
        node_rpc_url: Option<Url>,
    ) -> Result<DepositStatus> {
        let (deposit, token, amount) =
            evm_deposit_fields(self.fetch_swap_response(swap_id).await?)?;
        let deposit_address = parse_address(&deposit, "client_evm_address")?;
        let token_address = parse_address(token.as_wire_str(), "source_token_address")?;
        let min_token_units = parse_u256_dec(&amount, "source_amount")?;

        let node_rpc_url = match node_rpc_url {
            Some(url) => url,
            None => {
                let chain = token.chain().ok_or_else(|| {
                    Error::InvalidSwap(format!(
                        "swap `{swap_id}` deposit token `{}` has no known EVM chain — pass node_rpc_url explicitly",
                        token.as_wire_str(),
                    ))
                })?;
                let default = chain.default_node_rpc_url().ok_or_else(|| {
                    Error::InvalidSwap(format!(
                        "swap `{swap_id}` deposit chain `{}` has no default node RPC URL — pass node_rpc_url explicitly",
                        chain.as_wire_str(),
                    ))
                })?;
                Url::parse(default).map_err(|e| {
                    Error::InvalidSwap(format!(
                        "internal: default node RPC URL `{default}` for chain `{}` failed to parse: {e}",
                        chain.as_wire_str(),
                    ))
                })?
            }
        };

        let node: RootProvider = crate::evm::http_provider(node_rpc_url);
        let balance_calldata = IERC20::balanceOfCall {
            account: deposit_address,
        }
        .abi_encode();
        let balance_call = alloy::rpc::types::TransactionRequest::default()
            .to(token_address)
            .input(balance_calldata.into());

        let eth_bal = node
            .get_balance(deposit_address)
            .await
            .map_err(|e| Error::Transport(format!("eth_getBalance: {e}")))?;
        let token_bal_bytes = node
            .call(balance_call)
            .await
            .map_err(|e| Error::Transport(format!("balanceOf eth_call: {e}")))?;
        let token_bal = decode_token_balance(token_bal_bytes.as_ref())?;

        Ok(DepositStatus {
            source_token_balance: token_bal,
            source_token_required: min_token_units,
            native_balance_wei: eth_bal,
            has_sufficient_source_token: token_bal >= min_token_units,
        })
    }

    /// Poll until the gasless deposit address holds enough source token
    /// AND enough native gas. Real users send funds to the address out-
    /// of-band (wallet, exchange, hardware device); this helper lets a
    /// caller wait on that arrival before invoking
    /// [`Self::fund_swap_gasless`].
    ///
    /// Resolves the deposit address + required token amount from the
    /// swap response itself; the caller supplies their EVM node RPC URL
    /// and the gas headroom they want to require (in wei). Without a
    /// paymaster the SDK recommends ~0.001 ETH headroom for a typical
    /// USDC→tBTC userOp; with a paymaster, pass 0.
    ///
    /// Returns `Error::Timeout` if `timeout` elapses before both
    /// thresholds are met. 5s poll interval, fixed.
    #[tracing::instrument(name = "wait_for_deposit_funding", skip_all, fields(%swap_id, min_eth_wei, ?timeout))]
    pub async fn wait_for_deposit_funding(
        &self,
        swap_id: &str,
        node_rpc_url: &Url,
        min_eth_wei: u64,
        timeout: std::time::Duration,
    ) -> Result<()> {
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

        let poll = async {
            // Gasless deposit funding is EVM-only; the LN ↔ Arkade rails
            // don't have an EVM EOA deposit address at all.
            let (deposit, token, amount) =
                evm_deposit_fields(self.fetch_swap_response(swap_id).await?)?;
            let deposit_address = parse_address(&deposit, "client_evm_address")?;
            let token_address = parse_address(token.as_wire_str(), "source_token_address")?;
            let min_token_units = parse_u256_dec(&amount, "source_amount")?;
            let min_eth = U256::from(min_eth_wei);

            // Explicit `RootProvider` (defaults to Ethereum network) — the
            // generic param is otherwise ambiguous when only `Provider`
            // methods (no Network-specific filler) are touched.
            let node: RootProvider = crate::evm::http_provider(node_rpc_url.clone());
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
                let token_bal = decode_token_balance(token_bal_bytes.as_ref())?;
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
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        };
        tokio::time::timeout(timeout, poll).await.map_err(|_| {
            Error::Timeout(format!(
                "swap {swap_id} did not receive enough deposit funding within {timeout:?}"
            ))
        })?
    }

    async fn fetch_userop_calldata(&self, swap_id: &str) -> Result<UseropFundingCalldataResponse> {
        // GET /swap/{id}/swap-and-lock-calldata-userop — sidesteps the
        // `Endpoint` trait because the trait's `PATH` is static, and
        // this URL is per-swap.
        let url = self.swap_action_url(swap_id, "swap-and-lock-calldata-userop")?;
        let resp = self.api_request(reqwest::Method::GET, url).send().await?;
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

    if alloy::primitives::keccak256(alloy::sol_types::SolValue::abi_encode(&calls))
        != parse_b256(&backend.calls_hash, "calls_hash")?
    {
        return Err(Error::InvalidSwap("funding calls hash mismatch".into()));
    }
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

fn evm_deposit_fields(
    response: crate::types::GetSwapResponse,
) -> Result<(String, crate::types::TokenId, String)> {
    use crate::types::GetSwapResponse;
    match response {
        GetSwapResponse::EvmToArkade(r) => Ok((
            r.client_evm_address,
            r.source_token.token_id,
            r.source_amount,
        )),
        GetSwapResponse::EvmToBitcoin(r) => Ok((
            r.client_evm_address,
            r.source_token.token_id,
            r.source_amount,
        )),
        GetSwapResponse::EvmToLightning(r) => Ok((
            r.client_evm_address,
            r.source_token.token_id,
            r.source_amount,
        )),
        _ => Err(Error::InvalidSwap(
            "deposit funding requires an EVM-sourced swap".into(),
        )),
    }
}

fn decode_token_balance(bytes: &[u8]) -> Result<U256> {
    if bytes.len() != 32 {
        return Err(Error::Decode(
            "balanceOf must return exactly 32 bytes".into(),
        ));
    }
    Ok(U256::from_be_slice(bytes))
}
