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

use crate::types::Chain;
use crate::types::CreateEvmToArkadeSwapRequest;
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

/// Flat JSON body for `POST /swap/evm/arkade`. Flattens the public
/// `QuoteAmount` mutex into the spec's `amount_in` / `amount_out`
/// siblings. `token_address` is a `TokenId` (the public type's field
/// type) — its `From<String> / Into<String>` impls route it to the
/// correct wire address.
#[derive(Serialize)]
pub(crate) struct CreateEvmToArkadeSwapRequestWire {
    pub(crate) target_address: String,
    pub(crate) evm_chain_id: u64,
    pub(crate) token_address: TokenId,
    pub(crate) hash_lock: String,
    pub(crate) receiver_pk: String,
    pub(crate) user_address: String,
    pub(crate) user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) amount_in: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) amount_out: Option<u64>,
    pub(crate) gasless: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bridge_source_chain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bridge_source_token_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) referral_code: Option<String>,
    // Server wire field is `extra_fees`; SDK surface uses
    // `extra_fees_bps` so the unit is unambiguous at call sites.
    #[serde(rename = "extra_fees", skip_serializing_if = "Option::is_none")]
    pub(crate) extra_fees_bps: Option<u16>,
}

impl From<CreateEvmToArkadeSwapRequest> for CreateEvmToArkadeSwapRequestWire {
    fn from(r: CreateEvmToArkadeSwapRequest) -> Self {
        let (amount_in, amount_out) = match r.amount {
            QuoteAmount::Source(v) => (Some(v), None),
            QuoteAmount::Target(v) => (None, Some(v)),
        };
        Self {
            target_address: r.target_address,
            evm_chain_id: r.evm_chain_id,
            token_address: r.token_address,
            hash_lock: r.hash_lock,
            receiver_pk: r.receiver_pk,
            user_address: r.user_address,
            user_id: r.user_id,
            amount_in,
            amount_out,
            gasless: r.gasless,
            bridge_source_chain: r.bridge_source_chain,
            bridge_source_token_address: r.bridge_source_token_address,
            referral_code: r.referral_code,
            extra_fees_bps: r.extra_fees_bps,
        }
    }
}
