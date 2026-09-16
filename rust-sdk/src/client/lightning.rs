use super::Client;
use super::Swap;
use super::check_status;
use super::validate_sats;
use crate::Error;
use crate::Result;
use crate::types::EvmToLightningSwapResponse;
use crate::types::GetSwapResponse;
use crate::types::KnownChain;
use crate::types::LightningDestination;
use crate::types::LightningSendQuoteResponse;
use crate::types::TokenId;
use reqwest::Method;
use serde_json::Value;
use serde_json::json;

/// Shared destination encoding for Lightning sends. Field names differ between
/// the exact quote and creation endpoints; source-token amounts stay strings.
fn destination_fields(destination: LightningDestination, source_field: &str) -> Result<Value> {
    Ok(match destination {
        LightningDestination::Invoice(invoice) => {
            if invoice.is_empty() {
                return Err(Error::InvalidSwap(
                    "Lightning invoice must not be empty".into(),
                ));
            }
            json!({ "lightning_invoice": invoice })
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
    /// Quote a Lightning payout using the provider's fee for the actual destination.
    /// Sources are Arkade BTC or an EVM token on its matching chain.
    pub async fn get_lightning_send_quote(
        &self,
        source_chain: KnownChain,
        source_token: TokenId,
        destination: LightningDestination,
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
        if let Some(code) = &self.referral_code {
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
        let mut body = destination_fields(destination, "amount_in")?;
        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| Error::InvalidSigner("swap creation requires a signer".into()))?;
        let index = self.storage.next_key_index()?;
        let params = signer.derive_swap_params(index)?;
        body["evm_chain_id"] = json!(chain);
        body["token_address"] = json!(source);
        body["user_address"] = json!(signer.derive_evm_key(index)?.address);
        body["user_id"] = json!(hex::encode(params.user_id));
        body["gasless"] = json!(gasless);
        if let Some(code) = &self.referral_code {
            body["referral_code"] = json!(code);
        }
        if let Some(fees) = extra_fees_bps {
            body["extra_fees"] = json!(fees);
        }
        let response = self
            .api_request(Method::POST, self.url("swap/evm/lightning")?)
            .json(&body)
            .send()
            .await?;
        let swap: EvmToLightningSwapResponse = check_status(response).await?.json().await?;
        self.persist_swap(index, GetSwapResponse::EvmToLightning(swap.clone()))?;
        Ok(Swap::from_response(GetSwapResponse::EvmToLightning(swap)))
    }
}
