//! `Client::fund_swap_gasless` — the user-facing entry point for the
//! ERC-4337 + EIP-7702 gasless funding flow.
//!
//! Sits in its own file (rather than in `client.rs`) so the
//! feature-gated alloy machinery doesn't leak into the base SDK's
//! source. Rust inherent impls can span files within a crate, so the
//! method lands on [`Client`] as if it were declared in `client.rs`.

use crate::Client;
use crate::aa::abi::Call;
use crate::aa::bundler::BundlerCasing;
use crate::aa::bundler::BundlerClient;
use crate::aa::orchestrate::FundSwapClients;
use crate::aa::orchestrate::FundSwapInputs;
use crate::aa::orchestrate::FundSwapReceipt;
use crate::aa::orchestrate::fund_swap;
use crate::aa::paymaster::PaymasterClient;
use crate::error::Error;
use crate::error::Result;
use alloy::primitives::Address;
use alloy::primitives::B256;
use alloy::primitives::U256;
use alloy::providers::Provider;
use alloy::providers::RootProvider;
use serde::Deserialize;
use serde_json::Value;
use std::str::FromStr;
use url::Url;

/// Bundler + paymaster + node-RPC configuration the gasless flow needs.
///
/// All four URLs are required (Alchemy lets you point bundler +
/// paymaster + node at the same URL; other setups split them). The
/// `paymaster_context` is the paymaster-specific JSON — for Alchemy
/// Gas Manager: `serde_json::json!({ "policyId": "<uuid>" })`. Use
/// `serde_json::Value::Null` for paymasters that don't take one.
#[derive(Debug, Clone)]
pub struct AaConfig {
    pub bundler_url: Url,
    pub paymaster_url: Url,
    pub node_rpc_url: Url,
    pub paymaster_context: Value,
    pub bundler_casing: BundlerCasing,
}

impl Client {
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

        // 3. RPC clients.
        let bundler = BundlerClient::new(aa_config.bundler_url, aa_config.bundler_casing)?;
        let paymaster = PaymasterClient::new(aa_config.paymaster_url, aa_config.bundler_casing)?;
        let node = RootProvider::new_http(aa_config.node_rpc_url);
        let chain_id = node
            .get_chain_id()
            .await
            .map_err(|e| Error::Transport(format!("eth_chainId: {e}")))?;

        // 4. Map backend wire types onto the orchestration input.
        let inputs = build_fund_swap_inputs(backend, evm_key.secret_key, eoa_address, chain_id)?;

        // 5. Drive the orchestration.
        fund_swap(
            inputs,
            FundSwapClients {
                bundler: &bundler,
                paymaster: &paymaster,
                node: &node,
                paymaster_context: aa_config.paymaster_context,
            },
        )
        .await
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
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Api {
                status: status.as_u16(),
                message: body,
            });
        }
        resp.json::<UseropFundingCalldataResponse>()
            .await
            .map_err(|e| Error::Decode(format!("userop calldata response: {e}")))
    }
}

// ── backend response wire types ────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct UseropFundingCalldataResponse {
    coordinator_address: String,
    #[allow(dead_code)] // surfaced for callers but not consumed by orchestrate.
    permit2_address: String,
    source_token_address: String,
    source_amount: u64,
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
        source_amount: U256::from(backend.source_amount),
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
