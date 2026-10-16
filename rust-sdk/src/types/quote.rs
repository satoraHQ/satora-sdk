//! Quote request and response types.
//!
//! `QuoteRequest`'s on-the-wire shape lives in `crate::wire` — the
//! `#[serde(into = …)]` attribute below routes (de)serialization through it.

use super::chain::Chain;
use super::token::TokenId;
use crate::request::Endpoint;
use crate::request::PayloadKind;
use crate::wire::QuoteRequestWire;
use reqwest::Method;
use serde::Deserialize;
use serde::Serialize;

/// The user must specify exactly one of `source_amount` or `target_amount`;
/// this enum makes that invariant unrepresentable as invalid.
///
/// Amounts are in the smallest unit of the corresponding token (satoshis for
/// BTC, raw on-chain units for EVM tokens). Values are `u128` and serialize as
/// decimal strings so 18-decimal EVM token amounts can exceed `u64::MAX`.
///
/// `u128` is intentionally used instead of an EVM-specific `U256` in the public
/// Rust SDK API: it keeps the quote API chain-agnostic while still covering any
/// realistic ERC-20 swap amount. At 18 decimals, `u128::MAX` represents roughly
/// 340 billion whole tokens, far beyond practical quote/liquidity limits. The
/// backend still accepts wider decimal strings and uses `U256` internally where
/// EVM/DEX plumbing requires it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum QuoteAmount {
    Source(u128),
    Target(u128),
}

/// Parameters for `Client::get_quote` and `GET /quote`.
///
/// Serializes to the spec's flat query-parameter shape via the private
/// `QuoteRequestWire` (the `#[serde(into = …)]` attribute below). Callers
/// interact only with this struct.
#[derive(Clone, Debug, Serialize)]
#[serde(into = "QuoteRequestWire")]
pub struct QuoteRequest {
    pub source_chain: Chain,
    pub source_token: TokenId,
    pub target_chain: Chain,
    pub target_token: TokenId,
    pub amount: QuoteAmount,
    /// Optional CCTP bridge destination chain (e.g. `"Base"`, `"Solana"`).
    pub bridge_target_chain: Option<String>,
    /// Optional CCTP bridge source chain (e.g. `"Optimism"`).
    pub bridge_source_chain: Option<String>,
    /// `true` when Circle's forwarder must create the destination USDC token
    /// account (relevant for non-EVM destinations like Solana).
    pub bridge_recipient_setup: bool,
    /// Optional referral code for tracking.
    pub referral_code: Option<String>,
    /// Per-swap referral surcharge, in basis points.
    pub extra_fees_bps: Option<u16>,
    /// BOLT11 invoice, Lightning address or LNURL the swap will pay. For a
    /// Lightning target paid from Arkade, an EVM hub or native RBTC,
    /// `Client::get_quote` first asks `/quote/lightning-send` for the provider's
    /// fee on this destination.
    /// An invoice carries its own amount, so `amount` is ignored for it. Never
    /// sent to `/quote`.
    pub lightning_destination: Option<String>,
}

impl QuoteRequest {
    /// Minimal constructor — populates the four required fields and the
    /// amount, leaving the optional bridge / referral fields unset.
    pub fn new(
        source_chain: Chain,
        source_token: TokenId,
        target_chain: Chain,
        target_token: TokenId,
        amount: QuoteAmount,
    ) -> Self {
        Self {
            source_chain,
            source_token,
            target_chain,
            target_token,
            amount,
            bridge_target_chain: None,
            bridge_source_chain: None,
            bridge_recipient_setup: false,
            referral_code: None,
            extra_fees_bps: None,
            lightning_destination: None,
        }
    }

    pub fn with_lightning_destination(mut self, destination: impl Into<String>) -> Self {
        self.lightning_destination = Some(destination.into());
        self
    }
}

impl Endpoint for QuoteRequest {
    type Response = QuoteResponse;
    const METHOD: Method = Method::GET;
    const PATH: &'static str = "quote";
    const PAYLOAD: PayloadKind = PayloadKind::Query;
}

/// Response of `GET /quote`. Maps to the `QuoteResponse` component schema.
///
/// Amount fields come back as **strings** on the wire to side-step JavaScript
/// `Number` precision limits. Fee fields are plain integers.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct QuoteResponse {
    /// Exchange rate (decimal-as-string): how much target token per BTC.
    pub exchange_rate: String,
    /// Network fee (satoshis) — server-paid gas + BTC mining fee.
    pub network_fee: u64,
    /// Additional gas the server pays to execute the DEX swap on behalf of
    /// the user (`redeemAndExecute` via the coordinator contract).
    pub gasless_network_fee: u64,
    /// Protocol fee in satoshis.
    pub protocol_fee: u64,
    /// Protocol fee rate (e.g. `0.0025` for 0.25%).
    pub protocol_fee_rate: f64,
    /// Minimum BTC value of the swap, in satoshis.
    pub min_amount: u64,
    /// Maximum BTC value of the swap, in satoshis.
    pub max_amount: u64,
    /// Pre-calculated source amount in smallest unit of source token (pre-fee).
    pub source_amount: String,
    /// Pre-calculated target amount in smallest unit of target token (pre-fee).
    pub target_amount: String,
    /// What the user actually sends including all fees.
    pub net_source_amount: String,
    /// What the user actually receives after all fees.
    pub net_target_amount: String,
    /// CCTP bridge forwarding fee in USDC smallest units. Only present when
    /// `bridge_target_chain` was specified in the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_fee: Option<u64>,
}

/// Referral pricing applied to a pair's `fee_percentage` when composing a quote.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReferralRates {
    /// Replaces the pair fee when set (a negotiated partner rate).
    pub base_fee_rate: Option<f64>,
    /// Added on top of the (possibly replaced) base fee.
    pub extra_fee_rate: f64,
}

impl From<f64> for ReferralRates {
    fn from(extra_fee_rate: f64) -> Self {
        Self {
            base_fee_rate: None,
            extra_fee_rate,
        }
    }
}

impl From<&super::ReferralFeeResponse> for ReferralRates {
    fn from(response: &super::ReferralFeeResponse) -> Self {
        Self {
            base_fee_rate: response.base_fee_rate,
            extra_fee_rate: response.extra_fee_rate,
        }
    }
}

impl From<super::ReferralFeeResponse> for ReferralRates {
    fn from(response: super::ReferralFeeResponse) -> Self {
        Self::from(&response)
    }
}
