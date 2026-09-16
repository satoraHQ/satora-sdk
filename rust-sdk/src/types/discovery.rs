//! Response types matching the pinned server API.

use super::Chain;
use super::SwapStatus;
use super::TokenInfo;
use serde::Deserialize;
use serde::Serialize;

/// `TokenInfos` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TokenInfos {
    pub btc_tokens: Vec<TokenInfo>,
    pub evm_tokens: Vec<TokenInfo>,
}

/// `SwapStatusEntry` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SwapStatusEntry {
    pub id: String,
    pub status: SwapStatus,
}

/// `BulkStatusResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BulkStatusResponse {
    pub not_found: Vec<String>,
    pub statuses: Vec<SwapStatusEntry>,
}

/// `MtpResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MtpResponse {
    pub mtp: u64,
    pub tip_height: u64,
}

/// `SupportAgentInfo` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SupportAgentInfo {
    pub npub: String,
}

/// `SupportAgentsResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SupportAgentsResponse {
    pub agents: Vec<SupportAgentInfo>,
}

/// `ServiceStatus` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ServiceStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_height: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_time: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub healthy: bool,
}

/// `ServicesStatus` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ServicesStatus {
    pub arbitrum: ServiceStatus,
    pub arkade: ServiceStatus,
    pub bitcoin: ServiceStatus,
    pub ethereum: ServiceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightning: Option<ServiceStatus>,
    pub polygon: ServiceStatus,
}

/// `StatusResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StatusResponse {
    pub healthy: bool,
    pub services: ServicesStatus,
    #[serde(default)]
    pub swaps_paused: bool,
}

/// `SwapPairInfo` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SwapPairInfo {
    pub fee_percentage: f64,
    pub max_sats: u64,
    pub min_sats: u64,
    pub network_fee_sats: u64,
    pub source: Chain,
    pub target: Chain,
}

/// `SwapPairsResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SwapPairsResponse {
    pub pairs: Vec<SwapPairInfo>,
}

/// `NetworkFee` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NetworkFee {
    pub source_sats: u64,
    pub target_sats: u64,
}

/// `NetworkFeePairEntry` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NetworkFeePairEntry {
    pub fees: NetworkFee,
    pub source: Chain,
    pub target: Chain,
}

/// `NetworkFeesResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NetworkFeesResponse {
    pub pairs: Vec<NetworkFeePairEntry>,
}

/// `TokenRef` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TokenRef {
    pub address: String,
    pub decimals: u32,
    pub symbol: String,
}

/// `ChainConfigEntry` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChainConfigEntry {
    pub btc_peg_rate: String,
    pub btc_pegged_token: TokenRef,
    pub chain: Chain,
}

/// `ChainConfigResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChainConfigResponse {
    pub chains: Vec<ChainConfigEntry>,
}

/// `ReferralFeeResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReferralFeeResponse {
    pub extra_fee_rate: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referral_label: Option<String>,
}

/// `LightningSendQuoteResponse` in `openapi.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LightningSendQuoteResponse {
    pub exchange_rate: String,
    pub max_amount_sats: u64,
    pub min_amount_sats: u64,
    pub network_fee_sats: u64,
    pub protocol_fee_rate: f64,
    pub protocol_fee_sats: u64,
    pub source_amount: String,
    pub source_amount_sats: u64,
    pub target_amount_sats: u64,
}
