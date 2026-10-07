//! Response types matching the pinned server API.

use super::SwapStatus;
use super::TokenInfo;
use serde::Deserialize;
use serde::Serialize;

/// `BtcToArkadeSwapResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BtcToArkadeSwapResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arkade_claim_txid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arkade_fund_txid: Option<String>,
    pub arkade_htlc_script_version: i64,
    pub arkade_server_pk: String,
    pub arkade_vhtlc_address: String,
    pub asset_amount: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_claim_txid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_fund_txid: Option<String>,
    pub btc_htlc_address: String,
    pub btc_htlc_script_version: i64,
    pub btc_refund_locktime: i64,
    pub created_at: String,
    pub fee_sats: i64,
    pub hash_lock: String,
    pub id: String,
    pub network: String,
    pub sats_receive: i64,
    pub server_vhtlc_pk: String,
    pub source_amount: String,
    pub source_token: TokenInfo,
    pub status: SwapStatus,
    pub target_amount: String,
    pub target_arkade_address: String,
    pub target_token: TokenInfo,
    pub unilateral_claim_delay: i64,
    pub unilateral_refund_delay: i64,
    pub unilateral_refund_without_receiver_delay: i64,
    pub vhtlc_refund_locktime: i64,
}

/// `BitcoinToEvmSwapResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BitcoinToEvmSwapResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_target_chain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_target_token_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_claim_txid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_fund_txid: Option<String>,
    pub btc_hash_lock: String,
    pub btc_htlc_address: String,
    pub btc_htlc_script_version: i64,
    pub btc_refund_locktime: i64,
    pub btc_server_pk: String,
    pub chain: String,
    pub client_evm_address: String,
    pub created_at: String,
    pub evm_chain_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_claim_txid: Option<String>,
    pub evm_coordinator_address: String,
    pub evm_expected_sats: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_fund_txid: Option<String>,
    pub evm_hash_lock: String,
    pub evm_htlc_address: String,
    #[serde(default)]
    pub evm_htlc_kind: EvmHtlcKind,
    pub evm_htlc_version: i64,
    pub evm_refund_locktime: i64,
    pub fee_sats: i64,
    pub id: String,
    pub network: String,
    pub server_evm_address: String,
    pub source_amount: String,
    pub source_token: TokenInfo,
    pub status: SwapStatus,
    pub target_amount: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_evm_address: Option<String>,
    pub target_token: TokenInfo,
    pub wbtc_address: String,
}

/// `ArkadeToEvmSwapResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArkadeToEvmSwapResponse {
    pub arkade_htlc_script_version: i64,
    pub arkade_server_pk: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_target_chain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_target_token_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_claim_txid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_fund_txid: Option<String>,
    pub btc_vhtlc_address: String,
    pub chain: String,
    pub client_evm_address: String,
    pub created_at: String,
    pub evm_chain_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_claim_txid: Option<String>,
    pub evm_coordinator_address: String,
    pub evm_expected_sats: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_fund_txid: Option<String>,
    pub evm_htlc_address: String,
    #[serde(default)]
    pub evm_htlc_kind: EvmHtlcKind,
    pub evm_htlc_version: i64,
    pub evm_refund_locktime: i64,
    pub fee_sats: i64,
    pub hash_lock: String,
    pub id: String,
    pub network: String,
    pub receiver_pk: String,
    pub sender_pk: String,
    pub server_evm_address: String,
    pub source_amount: String,
    pub source_token: TokenInfo,
    pub status: SwapStatus,
    pub target_amount: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_evm_address: Option<String>,
    pub target_token: TokenInfo,
    pub unilateral_claim_delay: i64,
    pub unilateral_refund_delay: i64,
    pub unilateral_refund_without_receiver_delay: i64,
    pub vhtlc_refund_locktime: i64,
    pub wbtc_address: String,
}

/// HTLC family of an EVM lock. On EVM-target swaps it selects the EIP-712 domain and
/// the claim call; on EVM-source swaps it decides how the lock is funded and refunded.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EvmHtlcKind {
    /// `HTLCErc20` plus `HTLCCoordinator`: locks the chain's BTC-pegged token.
    #[default]
    Erc20,
    /// `HTLCNative` plus `HTLCNativeCoordinator`: locks the chain's native coin.
    Native,
}

/// `LightningToEvmSwapResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LightningToEvmSwapResponse {
    pub bolt11_invoice: String,
    #[serde(default)]
    pub evm_htlc_kind: EvmHtlcKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_target_chain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_target_token_address: Option<String>,
    pub chain: String,
    pub client_evm_address: String,
    pub created_at: String,
    pub evm_chain_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_claim_txid: Option<String>,
    pub evm_coordinator_address: String,
    pub evm_expected_sats: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_fund_txid: Option<String>,
    pub evm_htlc_address: String,
    pub evm_htlc_version: i64,
    pub evm_refund_locktime: i64,
    pub fee_sats: i64,
    pub hash_lock: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invoice_expires_at: Option<i64>,
    pub network: String,
    pub server_evm_address: String,
    pub source_amount: String,
    pub source_token: TokenInfo,
    pub status: SwapStatus,
    pub target_amount: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_evm_address: Option<String>,
    pub target_token: TokenInfo,
    pub wbtc_address: String,
}

/// `EvmToBitcoinSwapResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvmToBitcoinSwapResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_source_chain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_source_token_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_claim_txid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_fund_txid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_fund_vout: Option<i64>,
    pub btc_hash_lock: String,
    pub btc_htlc_address: String,
    pub btc_htlc_script_version: i64,
    pub btc_refund_locktime: i64,
    pub btc_server_refund_pk: String,
    pub btc_user_claim_pk: String,
    pub chain: String,
    pub client_evm_address: String,
    pub created_at: String,
    pub evm_chain_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_claim_txid: Option<String>,
    pub evm_coordinator_address: String,
    pub evm_expected_sats: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_fund_txid: Option<String>,
    pub evm_funding_confirmations: i64,
    pub evm_hash_lock: String,
    pub evm_htlc_address: String,
    #[serde(default)]
    pub evm_htlc_kind: EvmHtlcKind,
    pub evm_htlc_version: i64,
    pub evm_refund_locktime: i64,
    pub fee_sats: i64,
    pub gasless: bool,
    pub id: String,
    pub network: String,
    pub server_evm_address: String,
    pub source_amount: String,
    pub source_token: TokenInfo,
    pub status: SwapStatus,
    pub target_amount: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_btc_address: Option<String>,
    pub target_token: TokenInfo,
    pub wbtc_address: String,
}

/// `EvmToLightningSwapResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvmToLightningSwapResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_source_chain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_source_token_address: Option<String>,
    pub chain: String,
    pub client_evm_address: String,
    pub client_lightning_invoice: String,
    pub created_at: String,
    pub evm_chain_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_claim_txid: Option<String>,
    pub evm_coordinator_address: String,
    pub evm_expected_sats: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_fund_txid: Option<String>,
    pub evm_funding_confirmations: i64,
    pub evm_htlc_address: String,
    #[serde(default)]
    pub evm_htlc_kind: EvmHtlcKind,
    pub evm_htlc_version: i64,
    pub evm_refund_locktime: i64,
    pub fee_sats: i64,
    pub gasless: bool,
    pub hash_lock: String,
    pub id: String,
    pub invoice_expires_at: i64,
    pub lightning_payment_settled: bool,
    pub network: String,
    pub network_fee_sats: i64,
    pub server_evm_address: String,
    pub source_amount: String,
    pub source_token: TokenInfo,
    pub status: SwapStatus,
    pub target_amount: String,
    pub target_token: TokenInfo,
    pub wbtc_address: String,
}

/// `BitcoinToLightningSwapResponse` in `openapi.json`. The SDK reads this direction
/// but cannot create, fund or refund it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BitcoinToLightningSwapResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_claim_txid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_fund_txid: Option<String>,
    pub btc_hash_lock: String,
    pub btc_htlc_address: String,
    pub btc_htlc_script_version: i64,
    pub btc_refund_locktime: i64,
    pub btc_server_pk: String,
    pub client_lightning_invoice: String,
    pub created_at: String,
    pub fee_sats: i64,
    pub hash_lock: String,
    pub id: String,
    pub invoice_expires_at: i64,
    pub network: String,
    pub network_fee_sats: i64,
    pub source_amount: String,
    pub source_token: TokenInfo,
    pub status: SwapStatus,
    pub target_amount: String,
    pub target_token: TokenInfo,
}
