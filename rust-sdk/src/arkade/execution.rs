//! Validated multi-VTXO spending with restartable offchain submission.
use super::*;
use crate::SwapMetadata;
use crate::types::GetSwapResponse;
use ark_rs::core::server::Info;
use ark_rs::core::server::SubmitOffchainTxResponse;
use ark_rs::core::server::VirtualTxOutPoint;
use bitcoin::Psbt;
use bitcoin::ScriptBuf;
use bitcoin::Sequence;
use bitcoin::TapLeafHash;
use bitcoin::TapSighashType;
use bitcoin::TxOut;
use bitcoin::hashes::hash160;
use bitcoin::sighash::Prevouts;
use bitcoin::sighash::SighashCache;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;

pub type ArkadeFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send + 'a>>;

/// Injectable Arkade reader/executor. Returned PSBTs are checked against locally
/// built transactions before further signatures are added.
pub trait ArkadeBackend: Send + Sync {
    /// Fetch a spending virtual/checkpoint transaction for chain observation.
    fn virtual_transaction(&self, _id: Txid) -> ArkadeFuture<'_, Option<Psbt>> {
        Box::pin(async {
            Err(invalid(
                "Arkade backend does not expose virtual transactions",
            ))
        })
    }
    fn info(&self) -> ArkadeFuture<'_, Info>;
    fn vtxos(&self, address: ArkAddress) -> ArkadeFuture<'_, Vec<VirtualTxOutPoint>>;
    fn submit(
        &self,
        ark_tx: Psbt,
        checkpoints: Vec<Psbt>,
    ) -> ArkadeFuture<'_, SubmitOffchainTxResponse>;
    fn finalize(&self, txid: Txid, checkpoints: Vec<Psbt>) -> ArkadeFuture<'_, ()>;
    fn pending(
        &self,
        _intent: ark_rs::core::intent::Intent,
    ) -> ArkadeFuture<'_, Vec<ark_rs::core::server::PendingTx>> {
        Box::pin(async { Ok(vec![]) })
    }
}
impl ArkadeBackend for ArkadeWallet {
    fn virtual_transaction(&self, id: Txid) -> ArkadeFuture<'_, Option<Psbt>> {
        Box::pin(async move {
            let result = self
                .client
                .network_client()
                .get_virtual_txs(vec![id.to_string()], None, None)
                .await
                .map_err(transport)?;
            Ok(result
                .txs
                .into_iter()
                .find(|tx| tx.unsigned_tx.compute_txid() == id))
        })
    }

    fn pending(
        &self,
        intent: ark_rs::core::intent::Intent,
    ) -> ArkadeFuture<'_, Vec<ark_rs::core::server::PendingTx>> {
        Box::pin(async move {
            self.client
                .network_client()
                .get_pending_tx(intent)
                .await
                .map_err(transport)
        })
    }
    fn info(&self) -> ArkadeFuture<'_, Info> {
        Box::pin(async { self.client.server_info().await.map_err(transport) })
    }
    fn vtxos(&self, address: ArkAddress) -> ArkadeFuture<'_, Vec<VirtualTxOutPoint>> {
        Box::pin(async move {
            self.client
                .get_virtual_tx_outpoints(std::iter::once(address))
                .await
                .map_err(transport)
        })
    }
    fn submit(
        &self,
        ark: Psbt,
        checkpoints: Vec<Psbt>,
    ) -> ArkadeFuture<'_, SubmitOffchainTxResponse> {
        Box::pin(async move {
            self.client
                .network_client()
                .submit_offchain_transaction_request(ark, checkpoints)
                .await
                .map_err(transport)
        })
    }
    fn finalize(&self, txid: Txid, checkpoints: Vec<Psbt>) -> ArkadeFuture<'_, ()> {
        Box::pin(async move {
            self.client
                .network_client()
                .finalize_offchain_transaction(txid, checkpoints)
                .await
                .map_err(transport)?;
            Ok(())
        })
    }
}
pub(super) fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidSwap(message.into())
}
pub(super) fn decode(error: impl std::fmt::Display) -> Error {
    Error::Decode(error.to_string())
}
pub(super) fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArkadeSpend {
    Claim,
    Refund,
    CooperativeRefund,
}

#[derive(Clone, Debug)]
pub struct ArkadeSpendOptions {
    pub destination: String,
    pub timeout: Duration,
}

/// PSBT condition witnesses are removed before storage and re-derived on resume.
/// The server's public signatures and exact transactions are retained.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArkadePending {
    pub operation: ArkadeSpend,
    pub destination: String,
    pub amount_sats: u64,
    pub ark_tx: String,
    pub checkpoints: Vec<String>,
    pub submitted: bool,
    pub completed: bool,
}
impl std::fmt::Debug for ArkadePending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArkadePending")
            .field("operation", &self.operation)
            .field("submitted", &self.submitted)
            .field("completed", &self.completed)
            .finish_non_exhaustive()
    }
}

pub struct ArkadeHtlc {
    pub options: StrictVhtlcOptions,
    pub script: StrictVhtlcScript,
    pub network: Network,
    pub is_receiver: bool,
    pub expected_amount: Amount,
}
impl ArkadeHtlc {
    pub fn address(&self) -> ArkAddress {
        self.script.address()
    }
    pub(super) fn spend_script(&self, operation: ArkadeSpend) -> Result<ScriptBuf> {
        match operation {
            ArkadeSpend::Claim if self.is_receiver => Ok(self.options.claim_script()),
            ArkadeSpend::Refund if !self.is_receiver => {
                Ok(self.options.refund_without_receiver_script())
            }
            ArkadeSpend::CooperativeRefund if !self.is_receiver => Ok(self.options.refund_script()),
            _ => Err(invalid("Arkade spend direction does not match the swap")),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct VhtlcBalance {
    pub spendable_sats: u64,
    pub recoverable_sats: u64,
    pub spent_sats: u64,
    /// Observed spends, useful when another device completed the swap.
    pub spent_by: Vec<Txid>,
}
fn key(value: &str) -> Result<XOnlyPublicKey> {
    let bytes = hex::decode(value.trim_start_matches("0x")).map_err(decode)?;
    if bytes.len() == 33 {
        secp256k1::PublicKey::from_slice(&bytes)
            .map(|p| p.x_only_public_key().0)
            .map_err(decode)
    } else {
        XOnlyPublicKey::from_slice(&bytes).map_err(decode)
    }
}

impl Client {
    /// Reconstruct any swap's Arkade side and verify its address and locally owned key.
    pub fn arkade_htlc(&self, id: &str) -> Result<ArkadeHtlc> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        let params = self
            .signer
            .as_ref()
            .ok_or_else(|| invalid("signer missing"))?
            .derive_swap_params(stored.key_index)?;
        let own = key(&hex::encode(params.public_key))?;
        let is_receiver = matches!(
            stored.response,
            GetSwapResponse::BtcToArkade(_)
                | GetSwapResponse::EvmToArkade(_)
                | GetSwapResponse::LightningToArkade(_)
        );
        if !is_receiver
            && !matches!(
                stored.response,
                GetSwapResponse::ArkadeToEvm(_) | GetSwapResponse::ArkadeToLightning(_)
            )
        {
            return Err(invalid("swap has no Arkade HTLC"));
        }
        let btc = matches!(stored.response, GetSwapResponse::BtcToArkade(_));
        let value = serde_json::to_value(&stored.response).map_err(decode)?;
        let string = |field: &str| {
            value[field]
                .as_str()
                .ok_or_else(|| invalid(format!("missing {field}")))
        };
        let uint = |field: &str| {
            value[field]
                .as_u64()
                .ok_or_else(|| invalid(format!("invalid {field}")))
        };
        let network = match string("network")? {
            "bitcoin" | "mainnet" => Network::Bitcoin,
            "regtest" => Network::Regtest,
            "signet" => Network::Signet,
            "testnet" => Network::Testnet,
            _ => return Err(invalid("unknown Arkade network")),
        };
        if uint("arkade_htlc_script_version")? != 1 {
            return Err(invalid("unsupported Arkade script version"));
        }
        let raw = hex::decode(string("hash_lock")?.trim_start_matches("0x")).map_err(decode)?;
        let preimage_hash = match raw.len() {
            20 => ripemd160::Hash::from_slice(&raw).map_err(decode)?,
            32 => ripemd160::Hash::hash(&raw),
            _ => return Err(invalid("invalid Arkade hash lock")),
        };
        if is_receiver
            && preimage_hash.to_byte_array()
                != hash160::Hash::hash(&params.preimage).to_byte_array()
        {
            return Err(invalid("Arkade preimage hash mismatch"));
        }
        let (sender, receiver) = if btc {
            (key(string("server_vhtlc_pk")?)?, own)
        } else {
            (key(string("sender_pk")?)?, key(string("receiver_pk")?)?)
        };
        if if is_receiver { receiver } else { sender } != own {
            return Err(invalid("Arkade signer public key mismatch"));
        }
        let sequence = |field| -> Result<Sequence> {
            parse_sequence_number(i64::try_from(uint(field)?).map_err(decode)?).map_err(decode)
        };
        let options = StrictVhtlcOptions {
            sender,
            receiver,
            server: key(string("arkade_server_pk")?)?,
            preimage_hash,
            refund_locktime: u32::try_from(uint("vhtlc_refund_locktime")?).map_err(decode)?,
            unilateral_claim_delay: sequence("unilateral_claim_delay")?,
            unilateral_refund_delay: sequence("unilateral_refund_delay")?,
            unilateral_refund_without_receiver_delay: sequence(
                "unilateral_refund_without_receiver_delay",
            )?,
        };
        let script = StrictVhtlcScript::new(options.clone(), network)?;
        let address = value
            .get("btc_vhtlc_address")
            .or_else(|| value.get("arkade_vhtlc_address"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid("Arkade HTLC address missing"))?;
        if script.address().encode() != address {
            return Err(invalid("Arkade HTLC address mismatch"));
        }
        let expected_amount = Amount::from_sat(
            string(if is_receiver {
                "target_amount"
            } else {
                "source_amount"
            })?
            .parse()
            .map_err(decode)?,
        );
        if expected_amount == Amount::ZERO || expected_amount > Amount::MAX_MONEY {
            return Err(invalid("invalid Arkade swap amount"));
        }
        Ok(ArkadeHtlc {
            options,
            script,
            network,
            is_receiver,
            expected_amount,
        })
    }

    pub async fn vhtlc_balance(
        &self,
        id: &str,
        backend: &dyn ArkadeBackend,
    ) -> Result<VhtlcBalance> {
        let htlc = self.arkade_htlc(id)?;
        let info = backend.info().await?;
        validate_info(&htlc, &info)?;
        let vtxos = backend.vtxos(htlc.address()).await?;
        validate_vtxos(&htlc, &vtxos)?;
        let list = VtxoList::new(info.dust, vtxos);
        let sum = |items: Vec<&VirtualTxOutPoint>| -> Result<u64> {
            items.iter().try_fold(0u64, |n, u| {
                n.checked_add(u.amount.to_sat())
                    .ok_or_else(|| invalid("Arkade balance overflow"))
            })
        };
        Ok(VhtlcBalance {
            spendable_sats: sum(list.spendable_offchain().collect())?,
            recoverable_sats: sum(list.recoverable().collect())?,
            spent_sats: sum(list.spent().collect())?,
            spent_by: list
                .spent()
                .filter_map(|u| u.spent_by.or(u.settled_by))
                .collect(),
        })
    }

    /// Multi-input Arkade spend. Refund maturity is independently checked using
    /// the supplied Bitcoin reader; claims do not require a chain reader.
    pub async fn spend_arkade(
        &self,
        id: &str,
        backend: &dyn ArkadeBackend,
        chain: Option<&dyn crate::bitcoin::BitcoinBackend>,
        operation: ArkadeSpend,
        options: ArkadeSpendOptions,
    ) -> Result<ClaimReceipt> {
        tokio::time::timeout(options.timeout, async {
            let _execution = self.arkade_execution.lock().await;
            let stored = self
                .get_stored_swap(id)?
                .ok_or_else(|| invalid("swap record missing"))?;
            if stored.metadata.arkade_delegate.is_some() {
                return Err(invalid(
                    "a delegated Arkade spend is already persisted; reconcile it first",
                ));
            }
            let params = self
                .signer
                .as_ref()
                .ok_or_else(|| invalid("signer missing"))?
                .derive_swap_params(stored.key_index)?;
            let htlc = self.arkade_htlc(id)?;
            let script = htlc.spend_script(operation)?;
            let destination = ArkAddress::decode(&options.destination).map_err(decode)?;
            if destination.encode() != options.destination
                || options.destination.starts_with("ark1") != (htlc.network == Network::Bitcoin)
            {
                return Err(invalid("Arkade destination network or server mismatch"));
            }
            let info = backend.info().await?;
            validate_info(&htlc, &info)?;
            if destination.server() != info.signer_pk.x_only_public_key().0 {
                return Err(invalid("Arkade destination must use the active server key"));
            }
            let pair = Keypair::from_secret_key(
                &Secp256k1::new(),
                &secp256k1::SecretKey::from_slice(&params.secret).map_err(decode)?,
            );
            let preimage = (operation == ArkadeSpend::Claim).then_some(params.preimage);
            let resuming = stored.metadata.arkade_pending.is_some();
            let mut pending = if let Some(pending) = stored.metadata.arkade_pending {
                if pending.operation != operation || pending.destination != options.destination {
                    return Err(invalid("Arkade pending operation or destination differs"));
                }
                pending
            } else {
                if operation == ArkadeSpend::Refund {
                    let chain = chain
                        .ok_or_else(|| invalid("Arkade refunds require a Bitcoin chain reader"))?;
                    if chain.network() != htlc.network {
                        return Err(invalid("Arkade refund chain network mismatch"));
                    }
                    let address = Address::from_script(&htlc.script.script_pubkey(), htlc.network)
                        .map_err(decode)?;
                    let clock = chain.observe(&address).await?;
                    let lock = htlc.options.refund_locktime;
                    let mature = if lock < 500_000_000 {
                        clock.height >= lock
                    } else {
                        clock.median_time_past > lock
                    };
                    if !mature {
                        return Err(invalid("Arkade refund timelock has not matured"));
                    }
                }
                let vtxos = backend.vtxos(htlc.address()).await?;
                validate_vtxos(&htlc, &vtxos)?;
                let list = VtxoList::new(info.dust, vtxos);
                if list.recoverable().next().is_some() {
                    return Err(invalid(
                        "recoverable Arkade outputs require delegated settlement",
                    ));
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(decode)?
                    .as_secs();
                let vtxos: Vec<_> = list
                    .spendable_offchain_at(&info, i64::try_from(now).map_err(decode)?, |_| {
                        Some(htlc.options.server)
                    })
                    .collect();
                if vtxos.is_empty() {
                    return Err(invalid("no spendable Arkade HTLC outputs"));
                }
                let amount = vtxos.iter().try_fold(0u64, |sum, v| {
                    sum.checked_add(v.amount.to_sat())
                        .filter(|s| *s <= Amount::MAX_MONEY.to_sat())
                        .ok_or_else(|| invalid("Arkade amount overflow"))
                })?;
                if operation == ArkadeSpend::Claim && amount < htlc.expected_amount.to_sat() {
                    return Err(invalid("Arkade HTLC is underfunded"));
                }
                let control = htlc
                    .script
                    .taproot_spend_info()
                    .control_block(&(script.clone(), LeafVersion::TapScript))
                    .ok_or_else(|| invalid("Arkade control block missing"))?;
                let inputs: Vec<_> = vtxos
                    .iter()
                    .map(|v| {
                        VtxoInput::new(
                            script.clone(),
                            (operation == ArkadeSpend::Refund).then(|| {
                                bitcoin::absolute::LockTime::from_consensus(
                                    htlc.options.refund_locktime,
                                )
                            }),
                            control.clone(),
                            htlc.script.tapscripts(),
                            htlc.script.script_pubkey(),
                            v.amount,
                            v.outpoint,
                            vec![],
                        )
                    })
                    .collect();
                let OffchainTransactions {
                    mut ark_tx,
                    checkpoint_txs,
                } = build_offchain_transactions(
                    &[SendReceiver::bitcoin(destination, Amount::from_sat(amount))],
                    &destination,
                    &inputs,
                    &info,
                )
                .map_err(decode)?;
                sign_psbt(&mut ark_tx, &pair, preimage)?;
                let mut pending = ArkadePending {
                    operation,
                    destination: options.destination.clone(),
                    amount_sats: amount,
                    ark_tx: archive(&ark_tx),
                    checkpoints: checkpoint_txs.iter().map(archive).collect(),
                    submitted: false,
                    completed: false,
                };
                if operation == ArkadeSpend::CooperativeRefund {
                    self.cosign_arkade_refund(id, &mut pending, &htlc).await?;
                }
                self.save_arkade_pending(id, pending.clone())?;
                pending
            };
            let ark = restore(&pending.ark_tx, preimage)?;
            let txid = ark.unsigned_tx.compute_txid();
            if pending.completed {
                return Ok(ClaimReceipt {
                    ark_txid: txid,
                    claim_amount_sats: pending.amount_sats,
                });
            }
            let mut checkpoints = pending
                .checkpoints
                .iter()
                .map(|p| restore(p, preimage))
                .collect::<Result<Vec<_>>>()?;
            if !pending.submitted {
                let recovered = if resuming {
                    let intent = pending_intent(&htlc, &checkpoints, &pair, preimage)?;
                    backend
                        .pending(intent)
                        .await?
                        .into_iter()
                        .find(|p| p.ark_txid == txid)
                } else {
                    None
                };
                let response = match recovered {
                    Some(p) => SubmitOffchainTxResponse {
                        signed_ark_tx: p.signed_ark_tx,
                        signed_checkpoint_txs: p.signed_checkpoint_txs,
                    },
                    None => backend.submit(ark.clone(), checkpoints.clone()).await?,
                };
                verify_cosigned(&ark, &response.signed_ark_tx, htlc.options.server)?;
                if response.signed_checkpoint_txs.len() != checkpoints.len() {
                    return Err(invalid("Arkade returned the wrong checkpoint count"));
                }
                for (expected, received) in checkpoints
                    .iter()
                    .zip(response.signed_checkpoint_txs.iter())
                {
                    verify_cosigned(expected, received, htlc.options.server)?;
                }
                for (expected, received) in
                    checkpoints.iter_mut().zip(response.signed_checkpoint_txs)
                {
                    *expected = merge_cosigned(expected, &received, htlc.options.server)?;
                    sign_psbt(expected, &pair, preimage)?;
                }
                pending.checkpoints = checkpoints.iter().map(archive).collect();
                pending.submitted = true;
                self.save_arkade_pending(id, pending.clone())?;
            }
            backend.finalize(txid, checkpoints).await?;
            pending.completed = true;
            self.save_arkade_pending(id, pending.clone())?;
            Ok(ClaimReceipt {
                ark_txid: txid,
                claim_amount_sats: pending.amount_sats,
            })
        })
        .await
        .map_err(|_| Error::Timeout("Arkade spend timed out; resume the stored operation".into()))?
    }
    fn save_arkade_pending(&self, id: &str, pending: ArkadePending) -> Result<()> {
        self.update_swap_metadata(
            id,
            SwapMetadata {
                arkade_pending: Some(pending),
                ..Default::default()
            },
        )?;
        Ok(())
    }

    async fn cosign_arkade_refund(
        &self,
        id: &str,
        pending: &mut ArkadePending,
        htlc: &ArkadeHtlc,
    ) -> Result<()> {
        if !matches!(
            self.get_stored_swap(id)?
                .ok_or_else(|| invalid("swap missing"))?
                .response,
            GetSwapResponse::ArkadeToEvm(_)
        ) {
            return Err(invalid(
                "cooperative Arkade refund endpoint requires Arkade to EVM",
            ));
        }
        let mut url = self.base_url.join("/api/swap/arkade-evm/")?;
        url.path_segments_mut()
            .map_err(|_| invalid("invalid API URL"))?
            .pop_if_empty()
            .push(id)
            .push("collab-refund");
        let response = self
            .api_request(reqwest::Method::POST, url)
            .json(
                &serde_json::json!({"ark_tx":pending.ark_tx,"checkpoint_txs":pending.checkpoints}),
            )
            .send()
            .await?;
        #[derive(Deserialize)]
        struct Response {
            ark_tx: String,
            checkpoint_txs: Vec<String>,
        }
        let response: Response = crate::client::check_status(response).await?.json().await?;
        let original = restore(&pending.ark_tx, None)?;
        let signed = merge_cosigned(
            &original,
            &restore(&response.ark_tx, None)?,
            htlc.options.receiver,
        )?;
        if response.checkpoint_txs.len() != pending.checkpoints.len() {
            return Err(invalid("refund checkpoint count changed"));
        }
        let checkpoints = pending
            .checkpoints
            .iter()
            .zip(&response.checkpoint_txs)
            .map(|(original, signed)| {
                merge_cosigned(
                    &restore(original, None)?,
                    &restore(signed, None)?,
                    htlc.options.receiver,
                )
                .map(|p| archive(&p))
            })
            .collect::<Result<Vec<_>>>()?;
        pending.ark_tx = archive(&signed);
        pending.checkpoints = checkpoints;
        Ok(())
    }
}

pub(super) fn validate_info(htlc: &ArkadeHtlc, info: &Info) -> Result<()> {
    if info.network != htlc.network
        || (info.signer_pk.x_only_public_key().0 != htlc.options.server
            && !info
                .deprecated_signers
                .iter()
                .any(|s| s.pk.x_only_public_key().0 == htlc.options.server))
    {
        return Err(invalid("Arkade server identity or network mismatch"));
    }
    Ok(())
}
pub(super) fn validate_vtxos(htlc: &ArkadeHtlc, vtxos: &[VirtualTxOutPoint]) -> Result<()> {
    let mut seen = HashSet::new();
    for v in vtxos {
        if v.script != htlc.script.script_pubkey()
            || v.amount > Amount::MAX_MONEY
            || !v.assets.is_empty()
            || !seen.insert(v.outpoint)
        {
            return Err(invalid(
                "Arkade VTXO script, amount, assets or identity mismatch",
            ));
        }
    }
    Ok(())
}
pub(super) fn condition(input: &mut psbt::Input, preimage: Option<[u8; 32]>) {
    let key = psbt::raw::Key {
        type_value: 222,
        key: VTXO_CONDITION_KEY.to_vec(),
    };
    if let Some(preimage) = preimage {
        let mut data = vec![1, 32];
        data.extend(preimage);
        input.unknown.insert(key, data);
    }
}
pub(super) fn sign_psbt(psbt: &mut Psbt, pair: &Keypair, preimage: Option<[u8; 32]>) -> Result<()> {
    let prevouts: Vec<TxOut> = psbt
        .inputs
        .iter()
        .map(|input| {
            input
                .witness_utxo
                .clone()
                .ok_or_else(|| invalid("Arkade witness output missing"))
        })
        .collect::<Result<_>>()?;
    for (index, input) in psbt.inputs.iter_mut().enumerate() {
        if input.tap_scripts.len() != 1 {
            return Err(invalid("Arkade spend must select one script"));
        }
        let (_, (script, version)) = input
            .tap_scripts
            .first_key_value()
            .ok_or_else(|| invalid("Arkade spend script missing"))?;
        let leaf = TapLeafHash::from_script(script, *version);
        let sighash_type = input
            .sighash_type
            .map(|s| s.taproot_hash_ty())
            .transpose()
            .map_err(decode)?
            .unwrap_or(TapSighashType::Default);
        if !matches!(
            sighash_type,
            TapSighashType::Default | TapSighashType::All | TapSighashType::AllPlusAnyoneCanPay
        ) {
            return Err(invalid("unsupported Arkade signature mode"));
        }
        let sighash = SighashCache::new(&psbt.unsigned_tx)
            .taproot_script_spend_signature_hash(
                index,
                &Prevouts::All(&prevouts),
                leaf,
                sighash_type,
            )
            .map_err(decode)?;
        let signature = Secp256k1::new().sign_schnorr_no_aux_rand(
            &secp256k1::Message::from_digest(sighash.to_byte_array()),
            pair,
        );
        input.tap_script_sigs.insert(
            (pair.x_only_public_key().0, leaf),
            bitcoin::taproot::Signature {
                signature,
                sighash_type,
            },
        );
        condition(input, preimage);
    }
    Ok(())
}
pub(super) fn archive(psbt: &Psbt) -> String {
    let mut psbt = psbt.clone();
    for input in &mut psbt.inputs {
        input.unknown.remove(&psbt::raw::Key {
            type_value: 222,
            key: VTXO_CONDITION_KEY.to_vec(),
        });
        input.final_script_witness = None;
        input.final_script_sig = None;
    }
    psbt.to_string()
}
pub(super) fn restore(encoded: &str, preimage: Option<[u8; 32]>) -> Result<Psbt> {
    let mut psbt = Psbt::from_str(encoded).map_err(decode)?;
    for input in &mut psbt.inputs {
        condition(input, preimage);
    }
    Ok(psbt)
}
fn verify_cosigned(original: &Psbt, signed: &Psbt, cosigner: XOnlyPublicKey) -> Result<()> {
    merge_cosigned(original, signed, cosigner).map(|_| ())
}
pub(super) fn merge_cosigned(
    original: &Psbt,
    signed: &Psbt,
    cosigner: XOnlyPublicKey,
) -> Result<Psbt> {
    if original.unsigned_tx != signed.unsigned_tx || original.inputs.len() != signed.inputs.len() {
        return Err(invalid("cosigner changed the Arkade transaction"));
    }
    let mut merged = original.clone();
    let prevouts: Vec<_> = original
        .inputs
        .iter()
        .map(|i| {
            i.witness_utxo
                .clone()
                .ok_or_else(|| invalid("Arkade witness output missing"))
        })
        .collect::<Result<_>>()?;
    for (index, (expected, actual)) in original.inputs.iter().zip(&signed.inputs).enumerate() {
        if (actual.witness_utxo.is_some() && expected.witness_utxo != actual.witness_utxo)
            || (!actual.tap_scripts.is_empty() && expected.tap_scripts != actual.tap_scripts)
        {
            return Err(invalid("cosigner changed the Arkade spend proof"));
        }
        let (control, (script, version)) = expected
            .tap_scripts
            .first_key_value()
            .ok_or_else(|| invalid("Arkade script missing"))?;
        let leaf = TapLeafHash::from_script(script, *version);
        let sighash_type = expected
            .sighash_type
            .map(|s| s.taproot_hash_ty())
            .transpose()
            .map_err(decode)?
            .unwrap_or(TapSighashType::Default);
        let hash = SighashCache::new(&original.unsigned_tx)
            .taproot_script_spend_signature_hash(
                index,
                &Prevouts::All(&prevouts),
                leaf,
                sighash_type,
            )
            .map_err(decode)?;
        let message = secp256k1::Message::from_digest(hash.to_byte_array());
        let valid = |sig: &bitcoin::taproot::Signature| {
            sig.sighash_type == sighash_type
                && Secp256k1::new()
                    .verify_schnorr(&sig.signature, &message, &cosigner)
                    .is_ok()
        };
        let signature = if let Some(signature) = actual
            .tap_script_sigs
            .get(&(cosigner, leaf))
            .filter(|sig| valid(sig))
        {
            *signature
        } else {
            // Arkd may return a finalized PSBT with partial-signature metadata removed.
            let witness = actual
                .final_script_witness
                .as_ref()
                .ok_or_else(|| invalid("Arkade cosigner signature missing"))?;
            let stack: Vec<_> = witness.iter().collect();
            if stack.len() < 3
                || stack[stack.len() - 2] != script.as_bytes()
                || stack[stack.len() - 1] != control.serialize()
            {
                return Err(invalid("Arkade finalized spend proof mismatch"));
            }
            stack[..stack.len() - 2]
                .iter()
                .filter_map(|bytes| bitcoin::taproot::Signature::from_slice(bytes).ok())
                .find(valid)
                .ok_or_else(|| invalid("Arkade finalized cosigner signature invalid"))?
        };
        merged.inputs[index]
            .tap_script_sigs
            .insert((cosigner, leaf), signature);
    }
    Ok(merged)
}

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;

fn pending_intent(
    htlc: &ArkadeHtlc,
    checkpoints: &[Psbt],
    pair: &Keypair,
    preimage: Option<[u8; 32]>,
) -> Result<ark_rs::core::intent::Intent> {
    use ark_rs::core::intent::Input;
    use ark_rs::core::intent::IntentMessage;
    use ark_rs::core::intent::make_intent;
    let inputs = checkpoints
        .iter()
        .map(|checkpoint| {
            if checkpoint.inputs.len() != 1 || checkpoint.unsigned_tx.input.len() != 1 {
                return Err(invalid("invalid stored Arkade checkpoint"));
            }
            let input = &checkpoint.inputs[0];
            let (control, (script, _)) = input
                .tap_scripts
                .first_key_value()
                .ok_or_else(|| invalid("checkpoint script missing"))?;
            Ok(Input::new_with_extra_witness(
                checkpoint.unsigned_tx.input[0].previous_output,
                checkpoint.unsigned_tx.input[0].sequence,
                Some(checkpoint.unsigned_tx.lock_time),
                input
                    .witness_utxo
                    .clone()
                    .ok_or_else(|| invalid("checkpoint witness output missing"))?,
                htlc.script.tapscripts(),
                (script.clone(), control.clone()),
                false,
                false,
                vec![],
                preimage.map(|p| vec![p.to_vec()]).unwrap_or_default(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    make_intent(
        |_, message| {
            Ok(vec![(
                Secp256k1::new().sign_schnorr_no_aux_rand(&message, pair),
                pair.x_only_public_key().0,
            )])
        },
        |_, _| {
            Err(ark_rs::core::Error::ad_hoc(
                "unexpected onchain intent input",
            ))
        },
        inputs,
        vec![],
        IntentMessage::GetPendingTx { expire_at: 0 },
    )
    .map_err(decode)
}
