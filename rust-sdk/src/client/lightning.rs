use super::Client;
use super::Swap;
use super::check_status;
use super::validate_sats;
use crate::Error;
use crate::Result;
use crate::types::GetSwapResponse;
use crate::types::KnownChain;
use crate::types::LightningDestination;
use crate::types::LightningSendQuoteResponse;
use crate::types::TokenId;
use bitcoin::hashes::Hash;
use lightning_invoice::Bolt11Invoice;
use reqwest::Method;
use serde_json::Value;
use serde_json::json;
use std::str::FromStr;

/// The refusal of a Lightning swap whose invoice does not pay the hash its lock opens
/// with. Such a swap must be neither paid nor funded.
pub(crate) const INVOICE_HASH_MISMATCH: &str =
    "the Lightning invoice does not pay the swap's hash lock";

/// Parsed as the swap server parses it, so both accept the same invoices. The
/// signature is checked too.
pub(super) fn parse_invoice(invoice: &str) -> Result<Bolt11Invoice> {
    Bolt11Invoice::from_str(invoice)
        .map_err(|e| Error::InvalidSwap(format!("invalid BOLT11 invoice: {e}")))
}

/// The payment hash a BOLT11 invoice commits its payer to.
pub(crate) fn invoice_payment_hash(invoice: &str) -> Result<[u8; 32]> {
    Ok(parse_invoice(invoice)?.payment_hash().to_byte_array())
}

fn hash_lock_bytes(hash_lock: &str) -> Result<[u8; 32]> {
    let mut hash = [0; 32];
    hex::decode_to_slice(hash_lock.trim_start_matches("0x"), &mut hash)
        .map_err(|e| Error::InvalidSwap(format!("invalid hash lock: {e}")))?;
    Ok(hash)
}

/// Refuses a Lightning swap whose invoice does not pay `hash`. A Lightning payout locks
/// the client's funds under its `hash_lock`, which must then be `hash` as well.
fn check_lightning_swap(response: &GetSwapResponse, hash: &[u8; 32]) -> Result<()> {
    let (invoice, hash_lock) = match response {
        GetSwapResponse::LightningToArkade(r) => (&r.bolt11_invoice, None),
        GetSwapResponse::LightningToEvm(r) => (&r.bolt11_invoice, None),
        GetSwapResponse::ArkadeToLightning(r) => (&r.client_lightning_invoice, Some(&r.hash_lock)),
        GetSwapResponse::EvmToLightning(r) => (&r.client_lightning_invoice, Some(&r.hash_lock)),
        GetSwapResponse::BitcoinToLightning(r) => (&r.client_lightning_invoice, Some(&r.hash_lock)),
        _ => return Ok(()),
    };
    if invoice_payment_hash(invoice)? != *hash
        || hash_lock
            .map(|hash_lock| hash_lock_bytes(hash_lock))
            .transpose()?
            .is_some_and(|hash_lock| hash_lock != *hash)
    {
        return Err(Error::InvalidSwap(INVOICE_HASH_MISMATCH.into()));
    }
    Ok(())
}

/// Refuses a Lightning payout whose hash lock is not the payment hash of its invoice.
fn check_lightning_payout(response: &GetSwapResponse) -> Result<()> {
    let hash_lock = match response {
        GetSwapResponse::ArkadeToLightning(r) => &r.hash_lock,
        GetSwapResponse::EvmToLightning(r) => &r.hash_lock,
        GetSwapResponse::BitcoinToLightning(r) => &r.hash_lock,
        _ => return Ok(()),
    };
    check_lightning_swap(response, &hash_lock_bytes(hash_lock)?)
}

/// Refuses a Lightning swap the server returned at creation or recovery whose invoice
/// does not pay the hash its lock opens with: for a Lightning input, `own_hash`, the
/// hash the client sent and holds the preimage of, since the server mints the hold
/// invoice; for a payout, the swap's hash lock.
pub(super) fn check_new_lightning_swap(
    response: &GetSwapResponse,
    own_hash: &[u8; 32],
) -> Result<()> {
    match response {
        GetSwapResponse::LightningToArkade(_) | GetSwapResponse::LightningToEvm(_) => {
            check_lightning_swap(response, own_hash)
        }
        _ => check_lightning_payout(response),
    }
}

/// Shared destination encoding for Lightning sends. Field names differ between
/// the exact quote and creation endpoints; source-token amounts stay strings.
pub(super) fn destination_fields(
    destination: LightningDestination,
    source_field: &str,
) -> Result<Value> {
    Ok(match destination {
        LightningDestination::Invoice(invoice) => {
            if invoice.is_empty() {
                return Err(Error::InvalidSwap(
                    "Lightning invoice must not be empty".into(),
                ));
            }
            json!({ "lightning_invoice": invoice })
        }
        LightningDestination::InvoiceExpecting { invoice, sats } => {
            if invoice.is_empty() {
                return Err(Error::InvalidSwap(
                    "Lightning invoice must not be empty".into(),
                ));
            }
            validate_sats(sats.into())?;
            json!({ "lightning_invoice": invoice, "target_amount_sats": sats })
        }
        LightningDestination::Address { address, sats } => {
            validate_sats(sats.into())?;
            json!({ "lightning_address": address, "target_amount_sats": sats })
        }
        LightningDestination::Lnurl { lnurl, sats } => {
            validate_sats(sats.into())?;
            json!({ "lnurl": lnurl, "target_amount_sats": sats })
        }
        LightningDestination::AddressSendMax { address, amount } => {
            if amount == 0 {
                return Err(Error::InvalidSwap("source amount must be positive".into()));
            }
            json!({ "lightning_address": address, (source_field): amount.to_string() })
        }
        LightningDestination::LnurlSendMax { lnurl, amount } => {
            if amount == 0 {
                return Err(Error::InvalidSwap("source amount must be positive".into()));
            }
            json!({ "lnurl": lnurl, (source_field): amount.to_string() })
        }
    })
}

impl Client {
    /// Refuses a fresh server snapshot of swap `id` that names another swap, that changes
    /// the direction of the stored swap, or whose invoice does not pay the hash a stored
    /// Lightning swap locks under, so that the snapshot neither replaces the stored copy
    /// nor reaches the caller. That hash is the client's own for a Lightning input, and
    /// for a payout the payment hash of the stored invoice. This SDK checks an invoice
    /// before storing it, but one an older SDK stored may be unchecked.
    pub(super) fn check_swap_snapshot(&self, id: &str, fresh: &GetSwapResponse) -> Result<()> {
        // A snapshot relabelled as another swap would be checked against the wrong local
        // state, or against none. The server spells the UUID in lower case whatever the
        // request's case, and the client stores no other spelling. The store matches ids
        // exactly, so a snapshot in another spelling would escape the stored swap's checks.
        let id = id.to_ascii_lowercase();
        if fresh.id() != id {
            return Err(Error::InvalidSwap(format!(
                "the server returned swap {} for swap {id}",
                fresh.id()
            )));
        }
        let Some(stored) = self.get_stored_swap(&id)? else {
            // An index-only entry, from an older database or a store without records after
            // a restart, gets the invoice checks recovery gives a swap: there is no stored
            // invoice to compare with, but the key index gives the client's own hash.
            let Some(index) = self.storage.get_swap_key_index(&id)? else {
                return Ok(());
            };
            return match fresh {
                GetSwapResponse::LightningToArkade(_) | GetSwapResponse::LightningToEvm(_) => {
                    check_lightning_swap(fresh, &self.own_hash_lock(index)?)
                }
                _ => check_lightning_payout(fresh),
            };
        };
        if std::mem::discriminant(fresh) != std::mem::discriminant(&stored.response) {
            return Err(Error::InvalidSwap(
                "server swap details differ from the stored swap".into(),
            ));
        }
        let hash = match &stored.response {
            GetSwapResponse::LightningToArkade(_) | GetSwapResponse::LightningToEvm(_) => {
                self.own_hash_lock(stored.key_index)?
            }
            GetSwapResponse::ArkadeToLightning(r) => {
                invoice_payment_hash(&r.client_lightning_invoice)?
            }
            GetSwapResponse::EvmToLightning(r) => {
                invoice_payment_hash(&r.client_lightning_invoice)?
            }
            GetSwapResponse::BitcoinToLightning(r) => {
                invoice_payment_hash(&r.client_lightning_invoice)?
            }
            _ => return Ok(()),
        };
        check_lightning_swap(fresh, &hash)
    }

    fn own_hash_lock(&self, key_index: u32) -> Result<[u8; 32]> {
        Ok(self
            .signer
            .as_ref()
            .ok_or_else(|| {
                Error::InvalidSigner("checking a Lightning swap requires a signer".into())
            })?
            .derive_swap_params(key_index)?
            .hash_lock)
    }

    /// Quote a Lightning payout using the provider's fee for the actual destination.
    /// Sources are Arkade BTC or an EVM token on its matching chain.
    pub async fn get_lightning_send_quote(
        &self,
        source_chain: KnownChain,
        source_token: TokenId,
        destination: LightningDestination,
        extra_fees_bps: Option<u16>,
    ) -> Result<LightningSendQuoteResponse> {
        self.lightning_send_quote_with_referral(
            source_chain,
            source_token,
            destination,
            self.referral_code.as_deref(),
            extra_fees_bps,
        )
        .await
    }

    pub(super) async fn lightning_send_quote_with_referral(
        &self,
        source_chain: KnownChain,
        source_token: TokenId,
        destination: LightningDestination,
        referral_code: Option<&str>,
        extra_fees_bps: Option<u16>,
    ) -> Result<LightningSendQuoteResponse> {
        let is_arkade = source_chain == KnownChain::Arkade && source_token == TokenId::Btc;
        if !is_arkade
            && (source_chain.evm_chain_id().is_none()
                || source_token == TokenId::Btc
                || source_token
                    .chain()
                    .is_some_and(|chain| chain != source_chain))
        {
            return Err(Error::InvalidSwap(
                "Lightning send source must be Arkade BTC or a matching EVM token".into(),
            ));
        }
        if is_arkade {
            match &destination {
                LightningDestination::AddressSendMax { amount, .. }
                | LightningDestination::LnurlSendMax { amount, .. } => validate_sats(*amount)?,
                _ => {}
            }
        }
        let mut query = destination_fields(destination, "source_amount")?;
        query["source_chain"] = json!(source_chain.as_wire_str());
        if !is_arkade {
            query["source_token"] = json!(source_token.as_wire_str());
        }
        if let Some(code) = referral_code {
            query["ref"] = json!(code);
        }
        if let Some(fees) = extra_fees_bps {
            query["extra_fees"] = json!(fees);
        }
        let response = self
            .api_request(Method::GET, self.url("quote/lightning-send")?)
            .query(&query)
            .send()
            .await?;
        Ok(check_status(response).await?.json().await?)
    }

    /// Create an EVM → Lightning swap. The invoice owns the hash lock, so
    /// duplicate-invoice errors are returned without retrying with another key.
    /// A hash lock that is not the payment hash of the returned invoice, or a
    /// returned invoice other than the supplied one, is refused with
    /// [`Error::InvalidSwap`] before anything is stored.
    ///
    /// With `gasless`, the returned deposit address can be funded and relayed
    /// using `fund_swap_gasless` (requires the `gasless` feature).
    /// Otherwise the caller must submit the HTLC funding transaction.
    pub async fn create_evm_to_lightning_swap(
        &self,
        source: TokenId,
        destination: LightningDestination,
        gasless: bool,
        extra_fees_bps: Option<u16>,
    ) -> Result<Swap> {
        let chain = source
            .chain()
            .and_then(|c| c.evm_chain_id())
            .ok_or_else(|| {
                Error::InvalidSwap("EVM→Lightning requires a named EVM source token".into())
            })?;
        self.create_swap_request(
            crate::types::SwapCreation::EvmToLightning {
                source: crate::types::EvmAsset {
                    chain_id: chain,
                    token: source,
                },
                destination,
            },
            crate::types::SwapOptions {
                gasless: Some(gasless),
                extra_fees_bps,
                ..Default::default()
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every invoice in the shared Lightning vectors, with the payment hash
    /// `lncli decodepayreq` printed for it (BOLT 11 states the spec vector's).
    fn shared_invoices() -> Vec<(String, String)> {
        let vectors: Value =
            serde_json::from_str(include_str!("../../../fixtures/lightning-invoices.json"))
                .unwrap();
        let text = |value: &Value| value.as_str().unwrap().to_owned();
        let mut invoices: Vec<_> = vectors["invoices"]
            .as_object()
            .unwrap()
            .values()
            .map(|invoice| (text(&invoice["bolt11"]), text(&invoice["payment_hash"])))
            .collect();
        for swap in vectors["swaps"].as_object().unwrap().values() {
            let response = &swap["response"];
            let invoice = response
                .get("bolt11_invoice")
                .unwrap_or(&response["client_lightning_invoice"]);
            invoices.push((text(invoice), text(&swap["invoice_payment_hash"])));
        }
        invoices
    }

    #[test]
    fn reads_the_payment_hash_of_real_lnd_and_spark_invoices() {
        let invoices = shared_invoices();
        assert!(invoices.len() >= 8);
        for (invoice, hash) in invoices {
            assert_eq!(hex::encode(invoice_payment_hash(&invoice).unwrap()), hash);
            // A QR code carries the invoice in upper case.
            assert_eq!(
                hex::encode(invoice_payment_hash(&invoice.to_uppercase()).unwrap()),
                hash
            );
        }
    }

    #[test]
    fn refuses_an_invoice_the_server_would_not_parse() {
        let (invoice, _) = shared_invoices().remove(0);
        let mut mixed_case = invoice.clone();
        mixed_case.replace_range(0..1, "L");
        for invoice in [
            "lnbcrt1fixture",
            "",
            &mixed_case,
            &invoice[..invoice.len() - 1],
        ] {
            let error = invoice_payment_hash(invoice).unwrap_err();
            assert!(
                matches!(&error, Error::InvalidSwap(message) if message.starts_with("invalid BOLT11 invoice")),
                "{invoice}: {error:?}"
            );
        }
    }
}
