use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DexToken {
    Evm { chain_id: u64, address: String },
    Solana { address: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DexAmount {
    ExactIn(String),
    ExactOut(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenAmount {
    pub raw: String,
    pub decimals: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "router", rename_all = "lowercase")]
pub enum BridgeRate {
    Cctp {
        fee_token: DexToken,
        proportional_fee_millionths: String,
        flat: String,
        flat_with_setup: String,
    },
    Layerzero {
        /// The LayerZero messaging fee the server pays in ETH and charges in sats.
        /// A server released before it charged the fee omits it.
        #[serde(default = "zero_sats")]
        native_fee_sats: String,
    },
}

fn zero_sats() -> String {
    "0".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DexQuoteRequest {
    pub from: DexToken,
    pub to: DexToken,
    pub amount: DexAmount,
    pub slippage_bps: u16,
    pub cctp_recipient_setup: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DexQuoteResponse {
    pub expected_amount_in: TokenAmount,
    pub estimated_amount_out: TokenAmount,
    pub estimated_settlement_seconds: u32,
    pub router: String,
    pub cache_ttl_seconds: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_rate: Option<BridgeRate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_fee: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DexCalldataRequest {
    pub from: DexToken,
    pub to: DexToken,
    pub amount: DexAmount,
    pub sender: String,
    pub slippage_bps: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_recipient: Option<BridgeRecipient>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DexCall {
    pub target: String,
    pub data: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DexCallsPayload {
    Evm { calls: Vec<DexCall> },
}

/// Unsigned transaction data. Fetching this response does not authorize its calls.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DexCalldataResponse {
    pub chain: String,
    pub payload: DexCallsPayload,
    pub expected_amount_in: String,
    pub estimated_amount_out: String,
    pub estimated_settlement_seconds: u32,
    pub router: String,
}

/// Destination-chain CCTP mint recipient, matching the current server and TypeScript SDK.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BridgeRecipient {
    Evm {
        address: String,
    },
    Solana {
        address: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        wallet: Option<String>,
    },
}
