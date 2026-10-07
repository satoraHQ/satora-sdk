use super::Client;
use super::MAX_KEY_INDEX_GRIND_ATTEMPTS;
use super::Swap;
use super::check_status;
use super::is_preimage_collision;
use super::lightning::check_new_lightning_swap;
use super::lightning::destination_fields;
use super::lightning::parse_invoice;
use super::validate_sats;
use crate::Error;
use crate::Result;
use crate::SwapMetadata;
use crate::logging::sdk_log;
use crate::types::EvmAsset;
use crate::types::GetSwapResponse;
use crate::types::QuoteAmount;
use crate::types::SwapCreation;
use crate::types::SwapOptions;
use crate::types::TokenId;
use bitcoin::hashes::Hash;
use bitcoin::hashes::hash160;
use reqwest::Method;
use serde_json::Value;
use serde_json::json;

fn evm_address(address: &str) -> Result<()> {
    if address.len() != 42
        || !address.starts_with("0x")
        || !address[2..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::InvalidSwap(
            "expected a 20-byte 0x-prefixed EVM address".into(),
        ));
    }
    Ok(())
}

fn asset_fields(body: &mut Value, asset: &EvmAsset) -> Result<()> {
    if asset.chain_id == 0
        || asset.chain_id > i64::MAX as u64
        || asset.token == TokenId::Btc
        || asset
            .token
            .chain()
            .is_some_and(|chain| chain.evm_chain_id() != Some(asset.chain_id))
    {
        return Err(Error::InvalidSwap(
            "EVM token and chain do not match".into(),
        ));
    }
    evm_address(asset.token.as_wire_str())?;
    body["evm_chain_id"] = json!(asset.chain_id);
    body["token_address"] = json!(asset.token);
    Ok(())
}

fn amount_fields(body: &mut Value, amount: &QuoteAmount, evm_source: bool) -> Result<()> {
    let (field, amount, sats) = match amount {
        QuoteAmount::Source(v) => ("amount_in", *v, !evm_source),
        QuoteAmount::Target(v) => ("amount_out", *v, evm_source),
    };
    if amount == 0 {
        return Err(Error::InvalidSwap("amount must be positive".into()));
    }
    body[field] = if sats {
        validate_sats(amount)?;
        json!(amount as u64)
    } else {
        json!(amount.to_string())
    };
    Ok(())
}

fn nonempty(value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::InvalidSwap("destination must not be empty".into()));
    }
    Ok(())
}

/// The payer funds an EVM-sourced swap at `client_evm_address`, so it must be the
/// address this client sent, whose key it holds.
fn check_deposit_address(response: &GetSwapResponse, sent: &str) -> Result<()> {
    let returned = match response {
        GetSwapResponse::EvmToArkade(r) => &r.client_evm_address,
        GetSwapResponse::EvmToBitcoin(r) => &r.client_evm_address,
        GetSwapResponse::EvmToLightning(r) => &r.client_evm_address,
        _ => return Err(Error::Decode("unexpected swap direction".into())),
    };
    if !returned.eq_ignore_ascii_case(sent) {
        return Err(Error::InvalidSwap(format!(
            "the server returned deposit address {returned}, not the {sent} this client sent"
        )));
    }
    Ok(())
}

fn arkade_address(address: &str) -> Result<()> {
    let decoded = ark_rs::core::ArkAddress::decode(address)
        .map_err(|e| Error::InvalidSwap(format!("invalid Arkade address: {e}")))?;
    let normalized = address.to_ascii_lowercase();
    if !(normalized.starts_with("ark1") || normalized.starts_with("tark1"))
        || decoded.encode() != normalized
    {
        return Err(Error::InvalidSwap(
            "expected an ark/tark bech32m address".into(),
        ));
    }
    Ok(())
}

impl Client {
    /// Create any API swap direction with explicit rails and EVM chain IDs.
    /// Local metadata is persisted with the response before returning.
    ///
    /// A Lightning swap is checked before anything is stored, and refused with
    /// [`Error::InvalidSwap`] if its invoice does not pay the hash its lock opens
    /// with: for Lightning→Arkade and Lightning→EVM the server's hold invoice must
    /// pay the hash of this client's preimage, and for Arkade→Lightning and
    /// EVM→Lightning the returned hash lock must be the payment hash of the returned
    /// invoice, which must be the supplied one when an invoice was supplied. A
    /// supplied invoice that does not parse is refused before the request. For
    /// Lightning addresses and LNURLs the server resolves the invoice, so only its
    /// agreement with the hash lock is checked.
    ///
    /// For Bitcoin→Arkade and Bitcoin→EVM the HTLC is rebuilt from the client's
    /// keys and the response, as [`Self::bitcoin_htlc`] does. [`Error::InvalidSwap`]
    /// refuses a hash lock that is not HASH160 of this client's preimage, an unknown
    /// network, an unsupported script version or zero refund locktime, and a
    /// `btc_htlc_address` other than the rebuilt one; [`Error::Decode`] refuses a
    /// server key, hash lock or refund locktime that does not parse. Such a swap
    /// stays in storage for recovery; never fund its address.
    ///
    /// For EVM to Arkade, Bitcoin and Lightning the payer funds the returned
    /// `client_evm_address`, so it must be the address sent: the per-swap key's,
    /// or `SwapOptions::evm_address` when set. Any other, compared without regard
    /// to letter case, is refused with [`Error::InvalidSwap`] after the swap is
    /// stored, as above; never show or fund that address.
    pub async fn create_swap_request(
        &self,
        request: SwapCreation,
        options: SwapOptions,
    ) -> Result<Swap> {
        use SwapCreation::*;
        let mut body = json!({});
        let (route, evm_source, evm_target, invoice_owned) = match &request {
            BitcoinToArkade {
                sats_receive,
                target_address,
            } => {
                validate_sats((*sats_receive).into())?;
                arkade_address(target_address)?;
                body["sats_receive"] = json!(sats_receive);
                body["target_arkade_address"] = json!(target_address);
                body["btc_htlc_script_version"] = json!(1);
                body["arkade_htlc_script_version"] = json!(1);
                ("bitcoin/arkade", false, false, false)
            }
            LightningToArkade {
                amount,
                target_address,
            } => {
                arkade_address(target_address)?;
                let (field, sats) = match amount {
                    QuoteAmount::Source(v) => ("source_amount_sats", *v),
                    QuoteAmount::Target(v) => ("target_amount_sats", *v),
                };
                validate_sats(sats)?;
                body[field] = json!(sats as u64);
                body["target_arkade_address"] = json!(target_address);
                body["arkade_htlc_script_version"] = json!(1);
                ("lightning/arkade", false, false, false)
            }
            ArkadeToLightning { destination } => {
                body = destination_fields(destination.clone(), "source_amount_sats")?;
                if let Some(amount) = body.get("source_amount_sats") {
                    let sats = amount
                        .as_str()
                        .and_then(|s| s.parse::<u128>().ok())
                        .ok_or_else(|| Error::InvalidSwap("invalid source sats".into()))?;
                    validate_sats(sats)?;
                    body["source_amount_sats"] = json!(sats as u64);
                }
                body["arkade_htlc_script_version"] = json!(1);
                ("arkade/lightning", false, false, true)
            }
            BitcoinToEvm {
                target,
                amount,
                target_address,
            }
            | ArkadeToEvm {
                target,
                amount,
                target_address,
            }
            | LightningToEvm {
                target,
                amount,
                target_address,
            } => {
                asset_fields(&mut body, target)?;
                amount_fields(&mut body, amount, false)?;
                nonempty(target_address)?;
                if options
                    .bridge
                    .as_ref()
                    .is_some_and(|b| b.chain.eq_ignore_ascii_case("solana"))
                {
                    let decoded = bitcoin::base58::decode(target_address)
                        .map_err(|e| Error::InvalidSwap(e.to_string()))?;
                    if decoded.len() != 32 {
                        return Err(Error::InvalidSwap(
                            "Solana destination must be a 32-byte base58 key".into(),
                        ));
                    }
                } else {
                    evm_address(target_address)?;
                }
                body["target_address"] = json!(target_address);
                let route = match &request {
                    BitcoinToEvm { .. } => {
                        body["btc_htlc_script_version"] = json!(1);
                        "bitcoin/evm"
                    }
                    ArkadeToEvm { .. } => {
                        body["arkade_htlc_script_version"] = json!(1);
                        "arkade/evm"
                    }
                    _ => "lightning/evm",
                };
                (route, false, true, false)
            }
            EvmToBitcoin {
                source,
                amount,
                target_address,
            } => {
                asset_fields(&mut body, source)?;
                amount_fields(&mut body, amount, true)?;
                if let Some(address) = target_address {
                    address
                        .parse::<bitcoin::Address<bitcoin::address::NetworkUnchecked>>()
                        .map_err(|e| Error::InvalidSwap(e.to_string()))?;
                    body["target_address"] = json!(address);
                }
                body["btc_htlc_script_version"] = json!(1);
                ("evm/bitcoin", true, false, false)
            }
            EvmToArkade {
                source,
                amount,
                target_address,
            } => {
                arkade_address(target_address)?;
                asset_fields(&mut body, source)?;
                amount_fields(&mut body, amount, true)?;
                body["target_address"] = json!(target_address);
                body["arkade_htlc_script_version"] = json!(1);
                ("evm/arkade", true, false, false)
            }
            EvmToLightning {
                source,
                destination,
            } => {
                body = destination_fields(destination.clone(), "amount_in")?;
                asset_fields(&mut body, source)?;
                ("evm/lightning", true, false, true)
            }
        };
        for field in ["lightning_invoice", "lightning_address", "lnurl"] {
            if let Some(value) = body.get(field).and_then(Value::as_str) {
                nonempty(value)?;
            }
        }
        let supplied_invoice = body
            .get("lightning_invoice")
            .and_then(Value::as_str)
            .map(parse_invoice)
            .transpose()?;
        if let Some(description) = &options.invoice_description {
            if !matches!(request, LightningToArkade { .. } | LightningToEvm { .. })
                || description.len() > 639
            {
                return Err(Error::InvalidSwap(
                    "invoice description is only valid for Lightning inputs and at most 639 bytes"
                        .into(),
                ));
            }
            body["invoice_description"] = json!(description);
        }
        if let Some(code) = &self.referral_code {
            body["referral_code"] = json!(code);
        }
        if let Some(fees) = options.extra_fees_bps {
            body["extra_fees"] = json!(fees);
        }
        if evm_source || evm_target {
            body["gasless"] = json!(options.gasless.unwrap_or(evm_target));
        } else if options.gasless.is_some() || options.evm_address.is_some() {
            return Err(Error::InvalidSwap("EVM options require an EVM side".into()));
        }
        let mut metadata = SwapMetadata {
            target_address: body
                .get("target_address")
                .or_else(|| body.get("target_arkade_address"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            ..Default::default()
        };
        if let Some(bridge) = &options.bridge {
            if !evm_source && !evm_target {
                return Err(Error::InvalidSwap("bridge requires an EVM side".into()));
            }
            nonempty(&bridge.chain)?;
            nonempty(&bridge.token_address)?;
            if evm_source
                && (bridge.recipient.is_some()
                    || bridge.recipient_wallet.is_some()
                    || bridge.recipient_setup)
            {
                return Err(Error::InvalidSwap(
                    "recipient setup requires an outbound bridge".into(),
                ));
            }
            let side = if evm_source { "source" } else { "target" };
            body[format!("bridge_{side}_chain")] = json!(bridge.chain);
            body[format!("bridge_{side}_token_address")] = json!(bridge.token_address);
            if evm_target {
                body["bridge_recipient_setup"] = json!(bridge.recipient_setup);
            }
            metadata.bridge_recipient_setup = Some(bridge.recipient_setup);
            metadata.bridge_recipient = bridge.recipient.clone();
            metadata.bridge_recipient_wallet = bridge.recipient_wallet.clone();
        }
        if let Some(address) = &options.evm_address {
            evm_address(address)?;
        }
        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| Error::InvalidSigner("swap creation requires a signer".into()))?;
        let direction = if route == "bitcoin/arkade" {
            "btc_to_arkade".to_owned()
        } else {
            route.replace('/', "_to_")
        };
        let log = self
            .log
            .child("create/retry", "create.retry_on_hash_collision");
        for attempt in 1..=MAX_KEY_INDEX_GRIND_ATTEMPTS {
            let index = self.storage.next_key_index()?;
            let result = async {
                let params = signer.derive_swap_params(index)?;
                let pk = json!(hex::encode(params.public_key));
                body["user_id"] = json!(hex::encode(params.user_id));
                if !invoice_owned {
                    body["hash_lock"] = if matches!(request, BitcoinToArkade { .. }) {
                        json!(hash160::Hash::hash(&params.preimage).to_string())
                    } else {
                        json!(format!("0x{}", hex::encode(params.hash_lock)))
                    };
                }
                match &request {
                    BitcoinToArkade { .. } => {
                        body["claim_pk"] = pk.clone();
                        body["refund_pk"] = pk;
                    }
                    LightningToArkade { .. } | EvmToBitcoin { .. } => body["claim_pk"] = pk,
                    EvmToArkade { .. } => body["receiver_pk"] = pk,
                    BitcoinToEvm { .. } | ArkadeToEvm { .. } | ArkadeToLightning { .. } => {
                        body["refund_pk"] = pk
                    }
                    _ => {}
                }
                let mut deposit_address = None;
                if evm_source || evm_target {
                    let address = match &options.evm_address {
                        Some(a) => a.clone(),
                        None => signer.derive_evm_key(index)?.address,
                    };
                    body[if evm_source {
                        "user_address"
                    } else {
                        "claiming_address"
                    }] = json!(address);
                    deposit_address = evm_source.then_some(address);
                }
                let response = self
                    .api_request(Method::POST, self.url(&format!("swap/{route}"))?)
                    .json(&body)
                    .send()
                    .await?;
                let mut response: Value = check_status(response).await?.json().await?;
                response
                    .as_object_mut()
                    .ok_or_else(|| {
                        Error::Decode("swap creation response is not a JSON object".into())
                    })?
                    .insert("direction".into(), json!(direction));
                let response: GetSwapResponse =
                    serde_json::from_value(response).map_err(|e| Error::Decode(e.to_string()))?;
                // Fetching a swap looks its record up by the lower-case id the server
                // answers with, so a record stored under another spelling would escape
                // the checks of `check_swap_snapshot`.
                if response.id() != response.id().to_ascii_lowercase() {
                    return Err(Error::InvalidSwap(format!(
                        "the server returned swap id {} with upper-case letters",
                        response.id()
                    )));
                }
                check_new_lightning_swap(&response, &params.hash_lock)?;
                if let Some(supplied) = &supplied_invoice {
                    let returned = match &response {
                        GetSwapResponse::ArkadeToLightning(r) => &r.client_lightning_invoice,
                        GetSwapResponse::EvmToLightning(r) => &r.client_lightning_invoice,
                        _ => return Err(Error::Decode("unexpected swap direction".into())),
                    };
                    if parse_invoice(returned)? != *supplied {
                        return Err(Error::InvalidSwap(
                            "the swap pays a different Lightning invoice than the one supplied"
                                .into(),
                        ));
                    }
                }
                self.persist_swap_with_metadata(index, response.clone(), metadata.clone())?;
                if let Some(sent) = &deposit_address {
                    check_deposit_address(&response, sent)?;
                }
                if matches!(
                    response,
                    GetSwapResponse::BtcToArkade(_) | GetSwapResponse::BitcoinToEvm(_)
                ) {
                    // The payer funds this address, so it must be the script our keys
                    // rebuild. Unlike a refused Lightning swap, the record stays stored,
                    // unreturned, for recovery.
                    self.bitcoin_htlc(response.id())?;
                }
                Ok::<_, Error>(Swap::from(response))
            }
            .await;
            let error = match result {
                Ok(swap) => return Ok(swap),
                Err(error) => error,
            };
            sdk_log!(
                log,
                Debug,
                "create.retry.attempt_failed",
                "Swap creation attempt failed",
                "attempt" => attempt,
                "keyIndex" => index;
                error = error
            );
            if invoice_owned || !is_preimage_collision(&error) {
                return Err(error);
            }
            // TypeScript skips ten additional indices after a collision.
            for _ in 0..10 {
                self.storage.next_key_index()?;
            }
        }
        Err(Error::InvalidSwap(
            "exhausted unused swap key attempts; recover the wallet before retrying".into(),
        ))
    }
}
