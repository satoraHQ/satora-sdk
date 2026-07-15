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
/// BTC, raw on-chain units for EVM tokens).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum QuoteAmount {
    Source(u64),
    Target(u64),
}

/// Parameters for `GET /quote`.
///
/// Serializes to the spec's flat query-parameter shape via the private
/// [`QuoteRequestWire`] (the `#[serde(into = …)]` attribute below). Callers
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
        }
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
