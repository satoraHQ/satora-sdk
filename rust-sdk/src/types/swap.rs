//! Swap creation types — start of the swap-API surface.
//!
//! Currently models the `POST /swap/evm/arkade` endpoint (EVM ERC-20 →
//! Bitcoin on Arkade), which is the path the btcpayserver plugin needs.
//! Other swap directions (BTC→EVM, EVM→Lightning, …) will land here as the
//! plugin scope grows.

use super::chain::Chain;
use super::quote::QuoteAmount;
use super::token::TokenId;
use crate::request::Endpoint;
use crate::request::PayloadKind;
use crate::wire::CreateEvmToArkadeSwapRequestWire;
use crate::wire::CreateLightningToArkadeSwapRequestWire;
use reqwest::Method;
use serde::Deserialize;
use serde::Serialize;

/// State machine for a swap. Matches the `SwapStatus` schema.
///
/// Wire format is a single lowercase string (e.g. `"clientfunded"`).
/// Unknown values fall through to [`SwapStatus::Other`] so the SDK doesn't
/// hard-fail when the backend adds a new state.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(from = "String", into = "String")]
#[non_exhaustive]
pub enum SwapStatus {
    Pending,
    ClientFundingSeen,
    ClientFunded,
    ClientRefunded,
    ServerFunded,
    ClientRedeeming,
    ClientRedeemed,
    ServerRedeemed,
    ClientFundedServerRefunded,
    ClientRefundedServerFunded,
    ClientRefundedServerRefunded,
    Expired,
    ClientInvalidFunded,
    ClientFundedTooLate,
    ServerWontFund,
    ClientRedeemedAndClientRefunded,
    /// Unrecognised wire value, preserved verbatim.
    Other(String),
}

impl SwapStatus {
    /// Wire representation as expected by the Lendaswap API.
    pub fn as_wire_str(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::ClientFundingSeen => "clientfundingseen",
            Self::ClientFunded => "clientfunded",
            Self::ClientRefunded => "clientrefunded",
            Self::ServerFunded => "serverfunded",
            Self::ClientRedeeming => "clientredeeming",
            Self::ClientRedeemed => "clientredeemed",
            Self::ServerRedeemed => "serverredeemed",
            Self::ClientFundedServerRefunded => "clientfundedserverrefunded",
            Self::ClientRefundedServerFunded => "clientrefundedserverfunded",
            Self::ClientRefundedServerRefunded => "clientrefundedserverrefunded",
            Self::Expired => "expired",
            Self::ClientInvalidFunded => "clientinvalidfunded",
            Self::ClientFundedTooLate => "clientfundedtoolate",
            Self::ServerWontFund => "serverwontfund",
            Self::ClientRedeemedAndClientRefunded => "clientredeemedandclientrefunded",
            Self::Other(s) => s.as_str(),
        }
    }
}

impl From<String> for SwapStatus {
    fn from(s: String) -> Self {
        match s.as_str() {
            "pending" => Self::Pending,
            "clientfundingseen" => Self::ClientFundingSeen,
            "clientfunded" => Self::ClientFunded,
            "clientrefunded" => Self::ClientRefunded,
            "serverfunded" => Self::ServerFunded,
            "clientredeeming" => Self::ClientRedeeming,
            "clientredeemed" => Self::ClientRedeemed,
            "serverredeemed" => Self::ServerRedeemed,
            "clientfundedserverrefunded" => Self::ClientFundedServerRefunded,
            "clientrefundedserverfunded" => Self::ClientRefundedServerFunded,
            "clientrefundedserverrefunded" => Self::ClientRefundedServerRefunded,
            "expired" => Self::Expired,
            "clientinvalidfunded" => Self::ClientInvalidFunded,
            "clientfundedtoolate" => Self::ClientFundedTooLate,
            "serverwontfund" => Self::ServerWontFund,
            "clientredeemedandclientrefunded" => Self::ClientRedeemedAndClientRefunded,
            _ => Self::Other(s),
        }
    }
}

impl From<SwapStatus> for String {
    fn from(s: SwapStatus) -> Self {
        s.as_wire_str().to_string()
    }
}

/// Metadata for a token quoted in a swap response. Maps to the spec's
/// `TokenInfo` schema.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TokenInfo {
    pub token_id: TokenId,
    pub symbol: String,
    pub chain: Chain,
    pub name: String,
    pub decimals: u32,
}

/// Request body for `POST /swap/evm/arkade`.
///
/// User has an ERC-20 token on an EVM chain and wants to receive BTC on
/// Arkade. The exclusive amount mutex is encoded by [`QuoteAmount`]: pick
/// `Source` to specify what you send, `Target` for what you receive.
#[derive(Clone, Debug, Serialize)]
#[serde(into = "CreateEvmToArkadeSwapRequestWire")]
pub struct CreateEvmToArkadeSwapRequest {
    /// Arkade address where the user wants to receive BTC.
    pub target_address: String,
    /// Numeric EVM chain ID. Use [`super::chain::KnownChain::evm_chain_id`]
    /// to derive from a `KnownChain`.
    pub evm_chain_id: u64,
    /// ERC-20 source token on the EVM chain.
    pub token_address: TokenId,
    /// Hash lock (`0x`-prefixed 32-byte hex) — `SHA256(secret)` where the
    /// client retains `secret` until claim.
    pub hash_lock: String,
    /// User's Arkade VHTLC claim public key.
    pub receiver_pk: String,
    /// User's EVM address (sender of the ERC-20).
    pub user_address: String,
    /// Recovery ID derived from the user's wallet.
    pub user_id: String,
    /// Source amount (`Source`) or desired output sats (`Target`).
    pub amount: QuoteAmount,
    /// `true` to have the server submit the funding tx on the user's
    /// behalf (Permit2 relay).
    pub gasless: bool,
    /// Optional: CCTP source chain when the user's USDC originates
    /// elsewhere and hops to Arbitrum via CCTPv2 before the HTLC is
    /// created.
    pub bridge_source_chain: Option<String>,
    /// Optional: native USDC address on `bridge_source_chain`.
    pub bridge_source_token_address: Option<String>,
    /// Optional referral code.
    pub referral_code: Option<String>,
    /// Optional per-swap fee surcharge in basis points
    /// (0..=`max_extra_fee_bps` configured on the matching developer key).
    /// When `None`, the key's `default_extra_fee_bps` applies server-side.
    pub extra_fees_bps: Option<u16>,
}

impl CreateEvmToArkadeSwapRequest {
    /// Build the request with every caller-controlled field at once. The
    /// CCTP bridge fields stay defaulted to `None` because the SDK doesn't
    /// model bridged sources for create-swap yet — when it does, they'll
    /// land here as additional parameters.
    ///
    /// Use the high-level [`crate::Client::create_swap`] when the SDK can
    /// derive most of these for you.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        target_address: impl Into<String>,
        evm_chain_id: u64,
        token_address: TokenId,
        hash_lock: impl Into<String>,
        receiver_pk: impl Into<String>,
        user_address: impl Into<String>,
        user_id: impl Into<String>,
        amount: QuoteAmount,
        gasless: bool,
        referral_code: Option<String>,
        extra_fees_bps: Option<u16>,
    ) -> Self {
        Self {
            target_address: target_address.into(),
            evm_chain_id,
            token_address,
            hash_lock: hash_lock.into(),
            receiver_pk: receiver_pk.into(),
            user_address: user_address.into(),
            user_id: user_id.into(),
            amount,
            gasless,
            bridge_source_chain: None,
            bridge_source_token_address: None,
            referral_code,
            extra_fees_bps,
        }
    }
}

impl Endpoint for CreateEvmToArkadeSwapRequest {
    type Response = EvmToArkadeSwapResponse;
    const METHOD: Method = Method::POST;
    const PATH: &'static str = "swap/evm/arkade";
    const PAYLOAD: PayloadKind = PayloadKind::JsonBody;
}

/// Wire body for `POST /swap/lightning/arkade`. Server-side counterpart:
/// `swap::api::lightning_to_arkade::LightningToArkadeSwapRequest`.
/// `claim_pk` and `user_id` are hex pubkeys derived from the signer
/// (see [`crate::Client::create_lightning_to_arkade_swap`]).
#[derive(Clone, Debug, Serialize)]
#[serde(into = "CreateLightningToArkadeSwapRequestWire")]
#[non_exhaustive]
pub struct CreateLightningToArkadeSwapRequest {
    pub target_arkade_address: String,
    pub sats_receive: u64,
    pub claim_pk: String,
    pub hash_lock: String,
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referral_code: Option<String>,
}

impl CreateLightningToArkadeSwapRequest {
    pub fn new(
        target_arkade_address: impl Into<String>,
        sats_receive: u64,
        claim_pk: impl Into<String>,
        hash_lock: impl Into<String>,
        user_id: impl Into<String>,
        referral_code: Option<String>,
    ) -> Self {
        Self {
            target_arkade_address: target_arkade_address.into(),
            sats_receive,
            claim_pk: claim_pk.into(),
            hash_lock: hash_lock.into(),
            user_id: user_id.into(),
            referral_code,
        }
    }
}

impl Endpoint for CreateLightningToArkadeSwapRequest {
    type Response = LightningToArkadeSwapResponse;
    const METHOD: Method = Method::POST;
    const PATH: &'static str = "swap/lightning/arkade";
    const PAYLOAD: PayloadKind = PayloadKind::JsonBody;
}

/// Response from `POST /swap/evm/arkade`. Maps to the
/// `EvmToArkadeSwapResponse` component schema.
///
/// `created_at` is left as the raw RFC3339 string from the wire — keeping
/// it a `String` means we avoid pulling in `chrono` / `time` just for one
/// field, and FFI consumers can parse it themselves. Large monetary
/// quantities (`source_amount`, `target_amount`, `evm_expected_sats`) come
/// back as decimal strings to side-step JavaScript number precision.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EvmToArkadeSwapResponse {
    pub id: String,
    pub status: SwapStatus,
    pub fee_sats: u64,
    pub hash_lock: String,
    pub source_token: TokenInfo,
    pub target_token: TokenInfo,
    pub created_at: String,
    pub chain: String,
    pub evm_chain_id: u64,
    pub source_amount: String,
    pub target_amount: String,
    pub evm_expected_sats: String,
    pub evm_htlc_address: String,
    pub client_evm_address: String,
    pub server_evm_address: String,
    pub evm_refund_locktime: u64,
    pub btc_vhtlc_address: String,
    pub target_arkade_address: String,
    pub sender_pk: String,
    pub receiver_pk: String,
    pub arkade_server_pk: String,
    pub vhtlc_refund_locktime: u64,
    pub unilateral_claim_delay: u64,
    pub unilateral_refund_delay: u64,
    pub unilateral_refund_without_receiver_delay: u64,
    pub network: String,
    pub gasless: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_source_chain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_source_token_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_claim_txid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_fund_txid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_claim_txid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_fund_txid: Option<String>,
}

/// Response shape for `GET /swap/{id}`. Mirrors the server's
/// `direction`-tagged `GetSwapResponse` enum, but only carries the
/// variants the SDK can fully model today (EVM → Arkade and Lightning
/// → Arkade). Other directions on the wire will fail to deserialize
/// here with a clear serde error — fine for now, since the SDK can't
/// do anything useful with them either.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "direction", rename_all = "snake_case")]
pub enum GetSwapResponse {
    EvmToArkade(EvmToArkadeSwapResponse),
    LightningToArkade(LightningToArkadeSwapResponse),
}

impl GetSwapResponse {
    /// Status of the swap, regardless of direction. Useful for the
    /// polling loop in `Client::wait_for_swap_status`.
    pub fn status(&self) -> &SwapStatus {
        match self {
            Self::EvmToArkade(r) => &r.status,
            Self::LightningToArkade(r) => &r.status,
        }
    }
}

/// Response from `POST /swap/lightning/arkade`. Maps to the
/// `LightningToArkadeSwapResponse` component schema.
///
/// Amount strings come back over the wire as decimal strings (server
/// uses `#[serde(with = "serde_string")]` on `i64` for `source_amount`
/// / `target_amount`) — keeping them as `String` here matches the EVM
/// response shape and avoids JS-side precision issues. VHTLC params
/// (`vhtlc_refund_locktime`, `unilateral_*_delay`) are plain JSON
/// numbers on both sides.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LightningToArkadeSwapResponse {
    pub id: String,
    pub status: SwapStatus,
    pub fee_sats: u64,
    pub hash_lock: String,
    pub source_token: TokenInfo,
    pub target_token: TokenInfo,
    pub created_at: String,
    pub source_amount: String,
    pub target_amount: String,
    /// Bolt11 invoice the user must pay over Lightning.
    pub bolt11_invoice: String,
    pub boltz_swap_id: String,
    /// Arkade VHTLC address the server locks BTC at — the user
    /// offchain-spends from here via the preimage path.
    pub arkade_vhtlc_address: String,
    pub target_arkade_address: String,
    pub sender_pk: String,
    pub receiver_pk: String,
    pub arkade_server_pk: String,
    pub vhtlc_refund_locktime: u64,
    pub unilateral_claim_delay: u64,
    pub unilateral_refund_delay: u64,
    pub unilateral_refund_without_receiver_delay: u64,
    pub network: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arkade_fund_txid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arkade_claim_txid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btc_claim_txid: Option<String>,
}
