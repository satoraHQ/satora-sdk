//! Validated multi-VTXO spending with restartable offchain submission.
use super::*;
use crate::SwapMetadata;
use crate::logging::SdkLogger;
use crate::logging::sdk_log;
use crate::types::GetSwapResponse;
use ark_rs::core::server::Info;
use ark_rs::core::server::PendingTx;
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
    /// An empty answer makes an attempted submission resubmit, so return arkd's
    /// pending transactions for the intent.
    fn pending(
        &self,
        intent: ark_rs::core::intent::Intent,
    ) -> ArkadeFuture<'_, Vec<ark_rs::core::server::PendingTx>>;
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
    /// Claims only: how long before the refund locktime a claim may still reveal
    /// the preimage. Height locks convert this at 600 seconds per block.
    pub minimum_remaining_seconds: u64,
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
    /// Set before the first submit leaves this process. For a claim, arkd may hold
    /// the preimage from then on.
    #[serde(default)]
    pub attempted: bool,
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

/// How long past a VTXO's expiry before the SDK trusts that arkd, whose clock may
/// lag ours, sees it expired too: arkd then refuses it offchain and expects no
/// forfeit for it in a batch.
pub(super) const EXPIRY_GRACE_SECS: i64 = 120;

pub(super) fn expired_beyond_grace(vtxo: &VirtualTxOutPoint, now: i64) -> bool {
    now >= vtxo.expires_at.saturating_add(EXPIRY_GRACE_SECS)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JournalOutcome {
    /// arkd may still accept or finalize the journal.
    Live,
    /// arkd never accepted the journal and never will.
    Dead,
}

/// The journal's Ark txid and the VTXOs its checkpoints spend.
fn journal_spend(pending: &ArkadePending) -> Result<(Txid, Vec<OutPoint>)> {
    let txid = restore(&pending.ark_tx, None)?.unsigned_tx.compute_txid();
    let inputs = pending
        .checkpoints
        .iter()
        .map(
            |checkpoint| match restore(checkpoint, None)?.unsigned_tx.input.as_slice() {
                [input] => Ok(input.previous_output),
                _ => Err(invalid("invalid stored Arkade checkpoint")),
            },
        )
        .collect::<Result<_>>()?;
    Ok((txid, inputs))
}

/// What the indexer says about an offchain journal. arkd refuses a transaction with
/// a swept or expired input before signing it and marks inputs spent only once it
/// accepts, so unspent inputs that are swept or past expiry mean it was never
/// accepted. Every state the indexer cannot settle counts as live; that includes
/// unrolled inputs, so such a journal keeps failing at arkd rather than settling.
pub(super) fn journal_outcome(
    pending: &ArkadePending,
    vtxos: &[VirtualTxOutPoint],
    now: i64,
) -> Result<JournalOutcome> {
    if pending.submitted || pending.completed {
        return Ok(JournalOutcome::Live);
    }
    let (_, inputs) = journal_spend(pending)?;
    let mut dead = false;
    for outpoint in &inputs {
        let Some(vtxo) = vtxos.iter().find(|v| v.outpoint == *outpoint) else {
            return Ok(JournalOutcome::Live);
        };
        if vtxo.is_spent || vtxo.is_unrolled {
            return Ok(JournalOutcome::Live);
        }
        dead |= vtxo.is_swept || expired_beyond_grace(vtxo, now);
    }
    Ok(if dead {
        JournalOutcome::Dead
    } else {
        JournalOutcome::Live
    })
}

/// Whether the indexer shows every input of the journal spent by the journal itself.
pub(super) fn journal_accepted(
    pending: &ArkadePending,
    vtxos: &[VirtualTxOutPoint],
) -> Result<bool> {
    let (txid, inputs) = journal_spend(pending)?;
    Ok(!inputs.is_empty()
        && inputs.iter().all(|outpoint| {
            vtxos
                .iter()
                .any(|v| v.outpoint == *outpoint && v.is_spent && v.ark_txid == Some(txid))
        }))
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
        let vtxos = validate_vtxos(&htlc, backend.vtxos(htlc.address()).await?)?;
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
    /// the supplied Bitcoin reader; claims need one only under a height refund lock.
    ///
    /// [`Error::Transport`] and [`Error::Timeout`] leave the outcome unknown, and so
    /// does [`Error::Storage`] once arkd may hold the spend: repeat the call, which
    /// resumes from the journal. A persisted spend arkd refused because its inputs are
    /// unspent and swept or expired for two minutes fails with [`Error::InvalidSwap`]
    /// instead, and [`Client::settle_arkade`] then settles the VHTLC in a delegated
    /// batch, for a claim only while the claim window is open.
    pub async fn spend_arkade(
        &self,
        id: &str,
        backend: &dyn ArkadeBackend,
        chain: Option<&dyn crate::bitcoin::BitcoinBackend>,
        operation: ArkadeSpend,
        options: ArkadeSpendOptions,
    ) -> Result<ClaimReceipt> {
        self.spend_arkade_with(id, backend, chain, operation, options, false)
            .await
    }

    /// With `continue_only`, only a spend arkd already holds is finished; the
    /// destination in `options` is ignored and nothing is ever submitted.
    pub(super) async fn spend_arkade_with(
        &self,
        id: &str,
        backend: &dyn ArkadeBackend,
        chain: Option<&dyn crate::bitcoin::BitcoinBackend>,
        operation: ArkadeSpend,
        options: ArkadeSpendOptions,
        continue_only: bool,
    ) -> Result<ClaimReceipt> {
        tokio::time::timeout(options.timeout, async {
            let _execution = self.arkade_execution.lock().await;
            let stored = self
                .get_stored_swap(id)?
                .ok_or_else(|| invalid("swap record missing"))?;
            let htlc = self.arkade_htlc(id)?;
            if let Some(state) = &stored.metadata.arkade_delegate {
                // A journal the indexer hid past expiry may have been accepted after
                // all; its delegated intent can then never settle, so it finishes here.
                let accepted = match (&stored.metadata.arkade_pending, &state.commitment_txid) {
                    (Some(pending), None) => journal_accepted(
                        pending,
                        &validate_vtxos(&htlc, backend.vtxos(htlc.address()).await?)?,
                    )?,
                    _ => false,
                };
                if !accepted {
                    return Err(invalid(
                        "a delegated Arkade spend is already persisted; reconcile it first",
                    ));
                }
            }
            let params = self
                .signer
                .as_ref()
                .ok_or_else(|| invalid("signer missing"))?
                .derive_swap_params(stored.key_index)?;
            let log = self.arkade_log(id, &htlc, operation, false, &options.destination);
            let continue_log = self.arkade_log(id, &htlc, operation, true, &options.destination);
            // TypeScript's claim always tries continueArkadeClaim first, which logs
            // its own start; ours only continues a persisted or already spent claim.
            if continue_only {
                sdk_log!(continue_log, Info, "arkade.claim.continue_start", "Continuing Arkade claim");
            } else {
                let (event, message) = spend_event(operation, SpendStep::Start);
                sdk_log!(log, Info, event, message);
            }
            let mut continued = continue_only;
            let script = htlc.spend_script(operation)?;
            let destination = if continue_only {
                None
            } else {
                let destination = ArkAddress::decode(&options.destination).map_err(decode)?;
                if destination.encode() != options.destination
                    || options.destination.starts_with("ark1")
                        != (htlc.network == Network::Bitcoin)
                {
                    return Err(invalid("Arkade destination network or server mismatch"));
                }
                Some(destination)
            };
            let info = backend.info().await?;
            validate_info(&htlc, &info)?;
            if destination.is_some_and(|d| d.server() != info.signer_pk.x_only_public_key().0) {
                return Err(invalid("Arkade destination must use the active server key"));
            }
            let pair = Keypair::from_secret_key(
                &Secp256k1::new(),
                &secp256k1::SecretKey::from_slice(&params.secret).map_err(decode)?,
            );
            let preimage = (operation == ArkadeSpend::Claim).then_some(params.preimage);
            let mut recovered = None;
            let mut pending = if let Some(pending) = stored.metadata.arkade_pending {
                if operation == ArkadeSpend::Claim && !continue_only {
                    sdk_log!(continue_log, Info, "arkade.claim.continue_start", "Continuing Arkade claim");
                    continued = true;
                }
                // An attempted claim is fixed and may already sit at arkd, so it
                // finishes whichever destination it pays.
                let fixed = operation == ArkadeSpend::Claim && pending.attempted;
                if pending.operation != operation
                    || (destination.is_some() && !fixed && pending.destination != options.destination)
                {
                    return Err(invalid("Arkade pending operation or destination differs"));
                }
                if !pending.submitted && !pending.completed {
                    let vtxos = validate_vtxos(&htlc, backend.vtxos(htlc.address()).await?)?;
                    let now = i64::try_from(super::delegate::unix_now()?).map_err(decode)?;
                    if journal_outcome(&pending, &vtxos, now)? == JournalOutcome::Dead {
                        return Err(invalid(
                            "arkd can no longer accept the persisted Arkade spend; use Client::settle",
                        ));
                    }
                }
                // Once a submit was attempted arkd may already hold the preimage, and
                // refusing to finish would only strand the claim.
                if operation == ArkadeSpend::Claim && !fixed && !continue_only {
                    ensure_claim_window(&htlc, chain, options.minimum_remaining_seconds).await?;
                }
                if operation == ArkadeSpend::Claim {
                    sdk_log!(
                        continue_log,
                        Info,
                        "arkade.claim.continue_vtxos_found",
                        "Found VTXOs for pending Arkade claim",
                        "totalAmount" => pending.amount_sats.to_string(),
                        "vtxoCount" => pending.checkpoints.len()
                    );
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
                let vtxos = validate_vtxos(&htlc, backend.vtxos(htlc.address()).await?)?;
                let spent: Vec<_> = vtxos
                    .iter()
                    .filter(|v| {
                        v.is_spent
                            && v.ark_txid.is_some()
                            && v.settled_by.is_none()
                            && !v.is_unrolled
                    })
                    .collect();
                let queried = operation == ArkadeSpend::Claim && !spent.is_empty();
                let found = if queried {
                    if !continue_only {
                        sdk_log!(continue_log, Info, "arkade.claim.continue_start", "Continuing Arkade claim");
                    }
                    // Before the refund lock the VHTLC spends offchain only through the
                    // claim path or a refund our receiver key cosigned, which no SDK
                    // does. So arkd already holds the preimage and asking it for the
                    // pending claim reveals nothing new, even inside the margin.
                    if !claim_window_open(&htlc, chain, 0).await? {
                        return Err(invalid(
                            "Arkade HTLC spent offchain after its refund lock may have matured; not revealing the preimage to query arkd",
                        ));
                    }
                    let total = sum_sats(&spent)?;
                    sdk_log!(
                        continue_log,
                        Info,
                        "arkade.claim.continue_vtxos_found",
                        "Found VTXOs for pending Arkade claim",
                        "totalAmount" => total.to_string(),
                        "vtxoCount" => spent.len()
                    );
                    pending_claim(&htlc, &info, backend, &pair, params.preimage, &spent, &continue_log)
                        .await?
                } else if continue_only {
                    Err("no pending Arkade claim to continue")
                } else {
                    Err("no spendable Arkade HTLC outputs")
                };
                match found {
                    Ok((pending, found)) => {
                        self.save_arkade_pending(id, pending.clone())?;
                        recovered = Some(found);
                        continued = true;
                        pending
                    }
                    Err(unspendable) => {
                        let Some(destination) = destination else {
                            return Err(invalid(unspendable));
                        };
                        if queried {
                            sdk_log!(
                                log,
                                Debug,
                                "arkade.claim.no_pending",
                                "No pending Arkade claim found; submitting a new claim"
                            );
                        }
                        if operation == ArkadeSpend::Claim {
                            ensure_claim_window(&htlc, chain, options.minimum_remaining_seconds)
                                .await?;
                        }
                        let list = VtxoList::new(info.dust, vtxos);
                        if list.recoverable().next().is_some() {
                            return Err(invalid(
                                "recoverable Arkade outputs require delegated settlement; use Client::settle",
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
                            let (event, message) =
                                spend_event(operation, SpendStep::NoSpendableVtxos);
                            sdk_log!(
                                log,
                                Debug,
                                event,
                                message,
                                "vtxos" => list.all().map(vtxo_log).collect::<Vec<_>>()
                            );
                            return Err(invalid(unspendable));
                        }
                        let amount = sum_sats(&vtxos)?;
                        if operation == ArkadeSpend::Claim {
                            sdk_log!(
                                log,
                                Info,
                                "arkade.claim.vtxos_found",
                                "Found spendable VTXOs for Arkade claim",
                                "totalAmount" => amount.to_string(),
                                "vtxoCount" => vtxos.len()
                            );
                        } else {
                            sdk_log!(
                                log,
                                Info,
                                "arkade.refund.vtxos_found",
                                "Found spendable VTXOs for Arkade refund",
                                "totalAmount" => amount.to_string(),
                                "vtxoCount" => vtxos.len(),
                                "destinationAddressPPkScript" =>
                                    destination.to_p2tr_script_pubkey().to_hex_string()
                            );
                        }
                        if operation == ArkadeSpend::Claim {
                            self.ensure_claim_covered(id, &htlc, amount, &log, false)
                                .await?;
                        }
                        let inputs = vtxo_inputs(&htlc, operation, &script, &vtxos)?;
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
                            attempted: false,
                            submitted: false,
                            completed: false,
                        };
                        if operation == ArkadeSpend::CooperativeRefund {
                            self.cosign_arkade_refund(id, &mut pending, &htlc).await?;
                        }
                        self.save_arkade_pending(id, pending.clone())?;
                        pending
                    }
                }
            };
            let tail_log = if continued { &continue_log } else { &log };
            let ark = restore(&pending.ark_tx, preimage)?;
            let txid = ark.unsigned_tx.compute_txid();
            if pending.completed {
                return Ok(ClaimReceipt {
                    ark_txid: txid,
                    claim_amount_sats: pending.amount_sats,
                    destination: pending.destination,
                });
            }
            let mut checkpoints = pending
                .checkpoints
                .iter()
                .map(|p| restore(p, preimage))
                .collect::<Result<Vec<_>>>()?;
            if !pending.submitted {
                // The pending query's intent proof carries the preimage too.
                let recovered = match recovered {
                    Some(found) => Some(found),
                    None if pending.attempted => {
                        let intent = pending_intent(&htlc, &checkpoints, &pair, preimage)?;
                        let candidates = backend.pending(intent).await?;
                        let claim = operation == ArkadeSpend::Claim;
                        if claim && !candidates.is_empty() {
                            sdk_log!(
                                continue_log,
                                Info,
                                "arkade.claim.pending_txs_found",
                                "Found pending Arkade claim transactions",
                                "pendingTxCount" => candidates.len()
                            );
                        }
                        let found = candidates.into_iter().find(|p| p.ark_txid == txid);
                        if claim && found.is_none() && !continue_only {
                            sdk_log!(
                                log,
                                Debug,
                                "arkade.claim.no_pending",
                                "No pending Arkade claim found; submitting a new claim"
                            );
                        }
                        found
                    }
                    None => None,
                };
                let response = match recovered {
                    Some(p) => SubmitOffchainTxResponse {
                        signed_ark_tx: p.signed_ark_tx,
                        signed_checkpoint_txs: p.signed_checkpoint_txs,
                    },
                    None => {
                        if continue_only {
                            return Err(invalid("no pending Arkade claim to continue"));
                        }
                        if !pending.attempted {
                            pending.attempted = true;
                            self.save_arkade_pending(id, pending.clone())?;
                        }
                        backend.submit(ark.clone(), checkpoints.clone()).await?
                    }
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
                let (event, message) = spend_event(operation, SpendStep::CheckpointsSigned);
                sdk_log!(tail_log, Debug, event, message);
            }
            backend
                .finalize(txid, checkpoints)
                .await
                .inspect_err(|error| match (operation, continued) {
                    (ArkadeSpend::Claim, false) => sdk_log!(
                        tail_log,
                        Error,
                        "arkade.claim.finalize_failed",
                        "Failed to finalize Arkade claim";
                        error = error
                    ),
                    (ArkadeSpend::Claim, true) => sdk_log!(
                        tail_log,
                        Error,
                        "arkade.claim.continue_finalize_failed",
                        "Failed to finalize pending Arkade claim";
                        error = error
                    ),
                    (ArkadeSpend::Refund | ArkadeSpend::CooperativeRefund, _) => {}
                })?;
            pending.completed = true;
            self.save_arkade_pending(id, pending.clone())?;
            let (event, message) = spend_event(operation, SpendStep::Finalized);
            sdk_log!(tail_log, Info, event, message, "arkTxid" => txid.to_string());
            Ok(ClaimReceipt {
                ark_txid: txid,
                claim_amount_sats: pending.amount_sats,
                destination: pending.destination,
            })
        })
        .await
        .map_err(|_| Error::Timeout("Arkade spend timed out; resume the stored operation".into()))?
    }
    /// TypeScript's logger context for a spend. Continued claims log without the
    /// destination, as `continueArkadeClaim` does.
    fn arkade_log(
        &self,
        id: &str,
        htlc: &ArkadeHtlc,
        operation: ArkadeSpend,
        continued: bool,
        destination: &str,
    ) -> SdkLogger {
        let (module, name) = match (operation, continued) {
            (ArkadeSpend::Claim, false) => ("redeem/arkade", "arkade.claim"),
            (ArkadeSpend::Claim, true) => ("redeem/arkade", "arkade.claim.continue"),
            (ArkadeSpend::Refund, _) => ("refund/arkade", "arkade.refund"),
            (ArkadeSpend::CooperativeRefund, _) => ("refund/arkade", "arkade.refund.collab"),
        };
        let log = self
            .log
            .child(module, name)
            .swap_id(id)
            .with("vhtlcAddress", htlc.address().encode())
            .with("network", htlc.network.to_string());
        if operation == ArkadeSpend::Claim && continued {
            log
        } else {
            log.with("destinationAddress", destination)
        }
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

    /// Refuse to claim a VHTLC of `amount` sats below what the swap is owed, since
    /// the claim hands the server the preimage to the client's funds. An EVM to
    /// Arkade swap is owed the target it was created with, less any conversion
    /// shortfall its own EVM lock shows; any other swap its stored target.
    pub(super) async fn ensure_claim_covered(
        &self,
        id: &str,
        htlc: &ArkadeHtlc,
        amount: u64,
        log: &SdkLogger,
        delegated: bool,
    ) -> Result<()> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        if !matches!(stored.response, GetSwapResponse::EvmToArkade(_)) {
            return if amount < htlc.expected_amount.to_sat() {
                Err(invalid("Arkade HTLC is underfunded"))
            } else {
                Ok(())
            };
        }
        let created = stored
            .metadata
            .created_amounts
            .clone()
            .or_else(|| crate::storage::CreatedAmounts::of(&stored.response))
            .ok_or_else(|| invalid("swap amounts missing"))?;
        let target: u64 = created.target_amount.parse().map_err(decode)?;
        if amount >= target {
            return Ok(());
        }
        #[cfg(feature = "gasless")]
        {
            self.accept_conversion_shortfall(id, &stored, &created, amount, log, delegated)
                .await
        }
        #[cfg(not(feature = "gasless"))]
        {
            let _ = (log, delegated);
            Err(invalid(
                "Arkade HTLC is underfunded; accepting an EVM conversion shortfall needs an EVM reader, which needs the gasless feature",
            ))
        }
    }

    /// The daemon pays a short EVM lock its value in sats less the fees fixed at
    /// creation, so the claim is owed `target - (expected - locked)` with target
    /// and expected lock as created, each lock amount truncated to sats as the
    /// daemon does. The locked amount comes from the owner's own funding. Only
    /// the server states the expected lock, so a claim further below the target
    /// than the caller's limit is refused whatever the lock shows.
    #[cfg(feature = "gasless")]
    async fn accept_conversion_shortfall(
        &self,
        id: &str,
        stored: &crate::StoredSwap,
        created: &crate::storage::CreatedAmounts,
        amount: u64,
        log: &SdkLogger,
        delegated: bool,
    ) -> Result<()> {
        use crate::evm::U256;
        let (accepted, refresh_failed) = if delegated {
            (
                "delegate.settle.shortfall_accepted",
                "delegate.settle.refresh_failed",
            )
        } else {
            (
                "arkade.claim.shortfall_accepted",
                "arkade.claim.refresh_failed",
            )
        };
        let GetSwapResponse::EvmToArkade(response) = &stored.response else {
            return Err(invalid("not an EVM to Arkade swap"));
        };
        let target: u64 = created.target_amount.parse().map_err(decode)?;
        let limit = self
            .shortfall_limit
            .load(std::sync::atomic::Ordering::Relaxed);
        let short = target.saturating_sub(amount);
        if u128::from(short) * 10_000 > u128::from(target) * u128::from(limit) {
            return Err(invalid(format!(
                "Arkade HTLC is underfunded: the VHTLC holds {amount} sats, {short} below the target of {target}, which is more than this client's limit of {limit} basis points; raise it with Client::set_conversion_shortfall_limit"
            )));
        }
        let chain_id = response.evm_chain_id;
        let reader = self.evm_reader(chain_id).ok_or_else(|| {
            invalid(format!(
                "Arkade HTLC is underfunded; accepting an EVM conversion shortfall needs an EVM reader for chain {chain_id} (Client::set_evm_reader)"
            ))
        })?;
        let (token, decimals) = crate::evm::lock_asset(chain_id).ok_or_else(|| {
            invalid(format!(
                "Arkade HTLC is underfunded; the BTC token locked on EVM chain {chain_id} is unknown, so its shortfall cannot be read"
            ))
        })?;
        // The stored record follows the server only while the EVM lock it names
        // stays the same; the floor below never follows it.
        match self.get_swap_details(id).await {
            Ok(current) => {
                let coordinator = |response: &GetSwapResponse| match response {
                    GetSwapResponse::EvmToArkade(r) => {
                        Some(r.evm_coordinator_address.to_ascii_lowercase())
                    }
                    _ => None,
                };
                if current.id() != id
                    || coordinator(&current) != coordinator(&stored.response)
                    || crate::evm::EvmContext::from_response(&current)?
                        != crate::evm::EvmContext::from_response(&stored.response)?
                {
                    return Err(invalid("server swap details differ from the stored swap"));
                }
                self.persist_server_swap(stored.key_index, current)?;
            }
            Err(error) => sdk_log!(
                log,
                Warn,
                refresh_failed,
                "Could not refresh the swap before reading its EVM lock";
                error = error
            ),
        }
        let locked = self
            .owner_funded_evm_htlc(id, reader.as_ref(), token)
            .await?
            .amount;
        let scale = U256::from(10u64.pow(decimals.saturating_sub(8)));
        let sats = |units: U256| {
            u64::try_from(units / scale).map_err(|_| invalid("EVM amount out of range"))
        };
        let expected = sats(U256::from_str_radix(&created.evm_expected_sats, 10).map_err(decode)?)?;
        let locked = sats(locked)?;
        // The daemon funds neither of these.
        let fees = expected.checked_sub(target).ok_or_else(|| {
            invalid("Arkade HTLC is underfunded; the swap's target exceeds its expected EVM lock")
        })?;
        let floor = locked
            .checked_sub(fees)
            .filter(|floor| *floor > 0)
            .ok_or_else(|| {
                invalid("Arkade HTLC is underfunded; the swap's EVM lock does not cover its fees")
            })?;
        if amount < floor {
            return Err(invalid(format!(
                "Arkade HTLC is underfunded: {amount} sats, but its target of {target} adjusted by its EVM lock is {floor}"
            )));
        }
        sdk_log!(
            log,
            Info,
            accepted,
            "Accepting an Arkade claim below target by the swap's EVM conversion shortfall",
            "targetAmount" => target.to_string(),
            "vhtlcAmount" => amount.to_string(),
            "evmShortfallSats" => (target - floor).to_string()
        );
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
            GetSwapResponse::ArkadeToEvm(_) | GetSwapResponse::ArkadeToLightning(_)
        ) {
            return Err(invalid(
                "cooperative Arkade refund endpoint requires Arkade to EVM or Arkade to Lightning",
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

pub(super) async fn ensure_claim_window(
    htlc: &ArkadeHtlc,
    chain: Option<&dyn crate::bitcoin::BitcoinBackend>,
    margin_secs: u64,
) -> Result<()> {
    if claim_window_open(htlc, chain, margin_secs).await? {
        Ok(())
    } else {
        Err(invalid(
            "insufficient time to claim before the Arkade refund window",
        ))
    }
}
async fn claim_window_open(
    htlc: &ArkadeHtlc,
    chain: Option<&dyn crate::bitcoin::BitcoinBackend>,
    margin_secs: u64,
) -> Result<bool> {
    let lock = u64::from(htlc.options.refund_locktime);
    let open = if lock < 500_000_000 {
        let chain = chain.ok_or_else(|| {
            invalid("Arkade claims under a height refund lock require a Bitcoin chain reader")
        })?;
        if chain.network() != htlc.network {
            return Err(invalid("Arkade claim chain network mismatch"));
        }
        let address =
            Address::from_script(&htlc.script.script_pubkey(), htlc.network).map_err(decode)?;
        let height = u64::from(chain.observe(&address).await?.height);
        height.saturating_add(margin_secs.div_ceil(600)) < lock
    } else {
        // The daemon refunds VHTLCs once MTP passes the lock. Wall clock runs ahead
        // of MTP, so gating on it closes the claim window earlier.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(decode)?
            .as_secs();
        now.saturating_add(margin_secs) < lock
    };
    Ok(open)
}
#[derive(Clone, Copy)]
enum SpendStep {
    Start,
    NoSpendableVtxos,
    CheckpointsSigned,
    Finalized,
}
/// TypeScript's event name and message for a step of a claim or refund.
fn spend_event(operation: ArkadeSpend, step: SpendStep) -> (&'static str, &'static str) {
    let claim = operation == ArkadeSpend::Claim;
    match (step, claim) {
        (SpendStep::Start, true) => ("arkade.claim.start", "Starting Arkade claim"),
        (SpendStep::Start, false) => ("arkade.refund.start", "Starting Arkade refund"),
        (SpendStep::NoSpendableVtxos, true) => (
            "arkade.claim.no_spendable_vtxos",
            "No spendable VTXOs found at VHTLC address",
        ),
        (SpendStep::NoSpendableVtxos, false) => (
            "arkade.refund.no_spendable_vtxos",
            "No spendable VTXOs found at VHTLC address",
        ),
        (SpendStep::CheckpointsSigned, true) => (
            "arkade.claim.checkpoints_signed",
            "Checkpoint transactions signed",
        ),
        (SpendStep::CheckpointsSigned, false) => (
            "arkade.refund.checkpoints_signed",
            "Final checkpoint transactions signed",
        ),
        (SpendStep::Finalized, true) => ("arkade.claim.finalized", "Arkade claim finalized"),
        (SpendStep::Finalized, false) => ("arkade.refund.finalized", "Arkade refund finalized"),
    }
}
fn vtxo_log(vtxo: &VirtualTxOutPoint) -> serde_json::Value {
    serde_json::json!({
        "txid": vtxo.outpoint.txid.to_string(),
        "vout": vtxo.outpoint.vout,
        "value": vtxo.amount.to_sat(),
        "createdAt": vtxo.created_at,
        "expiresAt": vtxo.expires_at,
        "isPreconfirmed": vtxo.is_preconfirmed,
        "isSwept": vtxo.is_swept,
        "isUnrolled": vtxo.is_unrolled,
        "isSpent": vtxo.is_spent,
        "spentBy": vtxo.spent_by.map(|txid| txid.to_string()),
        "arkTxId": vtxo.ark_txid.map(|txid| txid.to_string()),
        "settledBy": vtxo.settled_by.map(|txid| txid.to_string()),
    })
}
fn sum_sats(vtxos: &[&VirtualTxOutPoint]) -> Result<u64> {
    vtxos.iter().try_fold(0u64, |sum, v| {
        sum.checked_add(v.amount.to_sat())
            .filter(|s| *s <= Amount::MAX_MONEY.to_sat())
            .ok_or_else(|| invalid("Arkade amount overflow"))
    })
}
fn vtxo_inputs(
    htlc: &ArkadeHtlc,
    operation: ArkadeSpend,
    script: &ScriptBuf,
    vtxos: &[&VirtualTxOutPoint],
) -> Result<Vec<VtxoInput>> {
    let control = htlc
        .script
        .taproot_spend_info()
        .control_block(&(script.clone(), LeafVersion::TapScript))
        .ok_or_else(|| invalid("Arkade control block missing"))?;
    Ok(vtxos
        .iter()
        .map(|v| {
            VtxoInput::new(
                script.clone(),
                (operation == ArkadeSpend::Refund).then(|| {
                    bitcoin::absolute::LockTime::from_consensus(htlc.options.refund_locktime)
                }),
                control.clone(),
                htlc.script.tapscripts(),
                htlc.script.script_pubkey(),
                v.amount,
                v.outpoint,
                vec![],
            )
        })
        .collect())
}

/// Ask arkd for pending transactions over VHTLC outputs it reports spent offchain
/// and return the first that is a claim of ours, with the journal to finish it,
/// or the reason arkd offered none.
async fn pending_claim(
    htlc: &ArkadeHtlc,
    info: &Info,
    backend: &dyn ArkadeBackend,
    pair: &Keypair,
    preimage: [u8; 32],
    spent: &[&VirtualTxOutPoint],
    log: &SdkLogger,
) -> Result<std::result::Result<(ArkadePending, PendingTx), &'static str>> {
    let script = htlc.spend_script(ArkadeSpend::Claim)?;
    let inputs = vtxo_inputs(htlc, ArkadeSpend::Claim, &script, spent)?;
    // A checkpoint depends only on its input, so any payee yields the same ones.
    let placeholder = htlc.address();
    let checkpoints = build_offchain_transactions(
        &[SendReceiver::bitcoin(
            placeholder,
            Amount::from_sat(sum_sats(spent)?),
        )],
        &placeholder,
        &inputs,
        info,
    )
    .map_err(decode)?
    .checkpoint_txs;
    let intent = pending_intent(htlc, &checkpoints, pair, Some(preimage))?;
    let candidates = backend.pending(intent).await?;
    if candidates.is_empty() {
        return Ok(Err(
            "Arkade HTLC already spent offchain and arkd holds no pending claim for it",
        ));
    }
    sdk_log!(
        log,
        Info,
        "arkade.claim.pending_txs_found",
        "Found pending Arkade claim transactions",
        "pendingTxCount" => candidates.len()
    );
    for candidate in candidates {
        if let Some(found) =
            match_pending_claim(htlc, info, pair, preimage, spent, &checkpoints, candidate)?
        {
            return Ok(Ok(found));
        }
    }
    Ok(Err(
        "none of arkd's pending transactions over the spent Arkade HTLC outputs is a standard claim signed by our receiver key",
    ))
}

/// Accept `candidate` only if it spends checkpoints of our own VHTLC outputs in
/// the standard claim shape and our receiver key signed it. That signature means
/// a client holding our seed chose its destination, so any destination is kept.
fn match_pending_claim(
    htlc: &ArkadeHtlc,
    info: &Info,
    pair: &Keypair,
    preimage: [u8; 32],
    spent: &[&VirtualTxOutPoint],
    checkpoints: &[Psbt],
    candidate: PendingTx,
) -> Result<Option<(ArkadePending, PendingTx)>> {
    let tx = &candidate.signed_ark_tx.unsigned_tx;
    if tx.compute_txid() != candidate.ark_txid {
        return Ok(None);
    }
    let mut order = Vec::with_capacity(tx.input.len());
    for input in &tx.input {
        let Some(index) = checkpoints
            .iter()
            .position(|c| input.previous_output == OutPoint::new(c.unsigned_tx.compute_txid(), 0))
        else {
            return Ok(None);
        };
        if order.contains(&index) || spent[index].ark_txid != Some(candidate.ark_txid) {
            return Ok(None);
        }
        order.push(index);
    }
    let Some(payee) = tx
        .output
        .first()
        .filter(|o| o.script_pubkey.is_p2tr())
        .and_then(|o| XOnlyPublicKey::from_slice(&o.script_pubkey.as_bytes()[2..]).ok())
    else {
        return Ok(None);
    };
    let destination = ArkAddress::new(
        htlc.network,
        info.signer_pk.x_only_public_key().0,
        bitcoin::key::TweakedPublicKey::dangerous_assume_tweaked(payee),
    );
    let selected: Vec<_> = order.iter().map(|i| spent[*i]).collect();
    let amount = sum_sats(&selected)?;
    let script = htlc.spend_script(ArkadeSpend::Claim)?;
    let inputs = vtxo_inputs(htlc, ArkadeSpend::Claim, &script, &selected)?;
    let Ok(OffchainTransactions {
        mut ark_tx,
        checkpoint_txs,
    }) = build_offchain_transactions(
        &[SendReceiver::bitcoin(destination, Amount::from_sat(amount))],
        &destination,
        &inputs,
        info,
    )
    else {
        return Ok(None);
    };
    if ark_tx.unsigned_tx != *tx
        || candidate.signed_checkpoint_txs.len() != checkpoint_txs.len()
        || verify_cosigned(&ark_tx, &candidate.signed_ark_tx, htlc.options.receiver).is_err()
    {
        return Ok(None);
    }
    // Later steps pair signed checkpoints with the journal's by position.
    let mut signed_checkpoints = Vec::with_capacity(checkpoint_txs.len());
    for checkpoint in &checkpoint_txs {
        let Some(signed) = candidate
            .signed_checkpoint_txs
            .iter()
            .find(|s| s.unsigned_tx == checkpoint.unsigned_tx)
        else {
            return Ok(None);
        };
        signed_checkpoints.push(signed.clone());
    }
    sign_psbt(&mut ark_tx, pair, Some(preimage))?;
    Ok(Some((
        ArkadePending {
            operation: ArkadeSpend::Claim,
            destination: destination.encode(),
            amount_sats: amount,
            ark_tx: archive(&ark_tx),
            checkpoints: checkpoint_txs.iter().map(archive).collect(),
            attempted: true,
            submitted: false,
            completed: false,
        },
        PendingTx {
            signed_checkpoint_txs: signed_checkpoints,
            ..candidate
        },
    )))
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
/// Asset VTXOs are dropped, not rejected: anyone can send one to the VHTLC
/// address, and spending it without an asset packet would burn the asset.
pub(super) fn validate_vtxos(
    htlc: &ArkadeHtlc,
    vtxos: Vec<VirtualTxOutPoint>,
) -> Result<Vec<VirtualTxOutPoint>> {
    let mut seen = HashSet::new();
    for v in &vtxos {
        if v.script != htlc.script.script_pubkey()
            || v.amount > Amount::MAX_MONEY
            || !seen.insert(v.outpoint)
        {
            return Err(invalid("Arkade VTXO script, amount or identity mismatch"));
        }
    }
    Ok(vtxos.into_iter().filter(|v| v.assets.is_empty()).collect())
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
        // Each Taproot signature commits to its own hash type. Default and All are
        // interchangeable on a script spend (arkd's cosigner uses Default where the
        // intent asks for All), but ANYONECANPAY must be kept: the batch appends a
        // connector input to forfeits after signing.
        let accepted = |candidate: TapSighashType| {
            candidate == sighash_type
                || matches!(
                    (sighash_type, candidate),
                    (
                        TapSighashType::Default | TapSighashType::All,
                        TapSighashType::Default | TapSighashType::All
                    )
                )
        };
        let valid = |sig: &bitcoin::taproot::Signature| {
            accepted(sig.sighash_type)
                && SighashCache::new(&original.unsigned_tx)
                    .taproot_script_spend_signature_hash(
                        index,
                        &Prevouts::All(&prevouts),
                        leaf,
                        sig.sighash_type,
                    )
                    .is_ok_and(|hash| {
                        Secp256k1::new()
                            .verify_schnorr(
                                &sig.signature,
                                &secp256k1::Message::from_digest(hash.to_byte_array()),
                                &cosigner,
                            )
                            .is_ok()
                    })
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
