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

use crate::strict_vhtlc::ARKADE_HTLC_SCRIPT_VERSION_STRICT;
use crate::types::Chain;
use crate::types::CreateArkadeToLightningSwapRequest;
use crate::types::CreateEvmToArkadeSwapRequest;
use crate::types::CreateLightningToArkadeSwapRequest;
use crate::types::LightningDestination;
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
    #[serde(rename = "extra_fees", skip_serializing_if = "Option::is_none")]
    pub(crate) extra_fees_bps: Option<u16>,
    pub(crate) source_chain: Chain,
    pub(crate) source_token: TokenId,
    pub(crate) target_chain: Chain,
    pub(crate) target_token: TokenId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target_amount: Option<String>,
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
            QuoteAmount::Source(v) => (Some(v.to_string()), None),
            QuoteAmount::Target(v) => (None, Some(v.to_string())),
        };
        Self {
            extra_fees_bps: r.extra_fees_bps,
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
    pub(crate) amount_in: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) amount_out: Option<u128>,
    pub(crate) gasless: bool,
    pub(crate) arkade_htlc_script_version: i64,
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
            QuoteAmount::Source(v) => (Some(v.to_string()), None),
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
            arkade_htlc_script_version: ARKADE_HTLC_SCRIPT_VERSION_STRICT,
            bridge_source_chain: r.bridge_source_chain,
            bridge_source_token_address: r.bridge_source_token_address,
            referral_code: r.referral_code,
            extra_fees_bps: r.extra_fees_bps,
        }
    }
}

/// Wire body for `POST /swap/lightning/arkade`. The public struct has
/// the same shape, but we keep a separate wire type so the
/// `skip_serializing_if` annotations stay an internal concern.
#[derive(Serialize)]
pub(crate) struct CreateLightningToArkadeSwapRequestWire {
    pub(crate) target_arkade_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_amount_sats: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target_amount_sats: Option<u128>,
    pub(crate) arkade_htlc_script_version: i64,
    #[serde(rename = "extra_fees", skip_serializing_if = "Option::is_none")]
    pub(crate) extra_fees_bps: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) invoice_description: Option<String>,
    pub(crate) claim_pk: String,
    pub(crate) hash_lock: String,
    pub(crate) user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) referral_code: Option<String>,
}

impl From<CreateLightningToArkadeSwapRequest> for CreateLightningToArkadeSwapRequestWire {
    fn from(r: CreateLightningToArkadeSwapRequest) -> Self {
        Self {
            target_arkade_address: r.target_arkade_address,
            source_amount_sats: match r.amount {
                QuoteAmount::Source(v) => Some(v),
                _ => None,
            },
            target_amount_sats: match r.amount {
                QuoteAmount::Target(v) => Some(v),
                _ => None,
            },
            arkade_htlc_script_version: ARKADE_HTLC_SCRIPT_VERSION_STRICT,
            extra_fees_bps: r.extra_fees_bps,
            invoice_description: r.invoice_description,
            claim_pk: r.claim_pk,
            hash_lock: r.hash_lock,
            user_id: r.user_id,
            referral_code: r.referral_code,
        }
    }
}

/// Wire body for `POST /swap/arkade/lightning`. The server accepts
/// exactly one of `lightning_invoice` / `lightning_address` / `lnurl`;
/// the `From` impl below flattens the SDK's typed
/// [`LightningDestination`] into the matching nullable fields.
/// `target_amount_sats` is set for the address, LNURL and expected-amount invoice
/// variants, and `source_amount_sats` for the send-max variants.
#[derive(Serialize)]
pub(crate) struct CreateArkadeToLightningSwapRequestWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lightning_invoice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lightning_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lnurl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target_amount_sats: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_amount_sats: Option<u128>,
    pub(crate) arkade_htlc_script_version: i64,
    #[serde(rename = "extra_fees", skip_serializing_if = "Option::is_none")]
    pub(crate) extra_fees_bps: Option<u16>,
    pub(crate) refund_pk: String,
    pub(crate) user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) referral_code: Option<String>,
}

impl From<CreateArkadeToLightningSwapRequest> for CreateArkadeToLightningSwapRequestWire {
    fn from(r: CreateArkadeToLightningSwapRequest) -> Self {
        let (lightning_invoice, lightning_address, lnurl, target_amount_sats, source_amount_sats) =
            match r.destination {
                LightningDestination::Invoice(s) => (Some(s), None, None, None, None),
                LightningDestination::InvoiceExpecting { invoice, sats } => {
                    (Some(invoice), None, None, Some(sats), None)
                }
                LightningDestination::Address { address, sats } => {
                    (None, Some(address), None, Some(sats), None)
                }
                LightningDestination::Lnurl { lnurl, sats } => {
                    (None, None, Some(lnurl), Some(sats), None)
                }
                LightningDestination::AddressSendMax { address, amount } => {
                    (None, Some(address), None, None, Some(amount))
                }
                LightningDestination::LnurlSendMax { lnurl, amount } => {
                    (None, None, Some(lnurl), None, Some(amount))
                }
            };
        Self {
            lightning_invoice,
            lightning_address,
            lnurl,
            target_amount_sats,
            source_amount_sats,
            arkade_htlc_script_version: ARKADE_HTLC_SCRIPT_VERSION_STRICT,
            extra_fees_bps: r.extra_fees_bps,
            refund_pk: r.refund_pk,
            user_id: r.user_id,
            referral_code: r.referral_code,
        }
    }
}
