//! Internal wire-format structs.
//!
//! Public types in [`crate::types`] are designed for ergonomics (e.g. the
//! mutex-encoding [`crate::types::QuoteAmount`] enum). Their on-the-wire
//! shape — the actual flat record that `serde_urlencoded` / `serde_json`
//! consumes — lives here. The pairing is wired up with
//! `#[serde(into = "wire::FooWire")] / from = "wire::FooWire"` on the public
//! type.
//!
//! Nothing in this module is re-exported from `lib.rs`; the module itself is
//! `pub(crate)`. Wire types stay invisible to downstream callers so we can
//! evolve them (rename fields, change types, drop them entirely) without it
//! counting as a breaking change in the public API.
//!

use crate::types::Chain;
use crate::types::QuoteAmount;
use crate::types::QuoteRequest;
use crate::types::TokenId;
use serde::Serialize;

/// Flat wire shape for `GET /quote`. Mirrors the OpenAPI query parameters
/// exactly: `source_amount` / `target_amount` are siblings (the
/// [`QuoteAmount`] mutex is flattened here), and the referral code is sent
/// under the spec's `ref` name.
#[derive(Serialize)]
pub(crate) struct QuoteRequestWire {
    pub(crate) source_chain: Chain,
    pub(crate) source_token: TokenId,
    pub(crate) target_chain: Chain,
    pub(crate) target_token: TokenId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_amount: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target_amount: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bridge_target_chain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bridge_source_chain: Option<String>,
    pub(crate) bridge_recipient_setup: bool,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub(crate) referral_code: Option<String>,
}

impl From<QuoteRequest> for QuoteRequestWire {
    fn from(r: QuoteRequest) -> Self {
        let (source_amount, target_amount) = match r.amount {
            QuoteAmount::Source(v) => (Some(v), None),
            QuoteAmount::Target(v) => (None, Some(v)),
        };
        Self {
            source_chain: r.source_chain,
            source_token: r.source_token,
            target_chain: r.target_chain,
            target_token: r.target_token,
            source_amount,
            target_amount,
            bridge_target_chain: r.bridge_target_chain,
            bridge_source_chain: r.bridge_source_chain,
            bridge_recipient_setup: r.bridge_recipient_setup,
            referral_code: r.referral_code,
        }
    }
}
