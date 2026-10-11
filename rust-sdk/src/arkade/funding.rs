//! Paying a swap's VHTLC from the client's own Arkade wallet under a journal.
use super::delegate::unix_now;
use super::execution::decode;
use super::execution::invalid;
use super::execution::validate_info;
use super::execution::validate_vtxos;
use super::offboard::LeftOut;
use super::offboard::expires_within;
use super::offboard::expiry_margin;
use super::offboard::vtxo_lifetime;
use super::*;
use crate::SwapMetadata;
use crate::logging::sdk_log;
use crate::types::GetSwapResponse;
use crate::types::SwapStatus;
use ark_rs::core::coin_select::VirtualTxOutPoint as Coin;
use ark_rs::core::coin_select::select_vtxos;
use ark_rs::core::server::Info;
use ark_rs::core::server::VirtualTxOutPoint;
use bitcoin::Psbt;
use bitcoin::Script;
use bitcoin::ScriptBuf;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;

/// A VHTLC payment from the wallet. The inputs are recorded before the payment can
/// leave this process, and the Ark txid once arkd finalized it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArkadeFunding {
    /// Wallet VTXOs the payment spends, as `txid:vout`.
    pub inputs: Vec<String>,
    /// The VHTLC address the payment pays; `None` once a journal shown never paid has
    /// been emptied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    pub txid: Option<String>,
}

const UNRESOLVED_FUNDING: &str = "an Ark transaction that spent inputs of a previous VHTLC funding has no visible output at the VHTLC; it may be that payment, so retry later and never pay the VHTLC by other means";
const MISSING_INPUT: &str =
    "arkd does not know an input of a previous VHTLC funding; never pay the VHTLC by other means";

/// A journaled input as arkd reports it now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JournaledInput {
    Spendable,
    /// Unspent but not spendable offchain (expired, swept, unrolled or under a retired
    /// signer), so this client cannot spend it in a payment.
    Unspendable,
    /// Spent outside a batch, by this Ark transaction or by one arkd does not name.
    Spent(Option<Txid>),
    /// Spent by an Ark transaction that pays nothing to the VHTLC.
    SpentElsewhere,
    /// Forfeited into a batch, which no offchain payment is.
    Settled,
    Missing,
}

/// Classify the journaled inputs from arkd's VTXOs for them and the outpoints the
/// wallet can spend offchain now.
pub(super) fn classify_inputs(
    journaled: &[OutPoint],
    vtxos: &[VirtualTxOutPoint],
    spendable: &HashSet<OutPoint>,
) -> Vec<(OutPoint, JournaledInput)> {
    journaled
        .iter()
        .map(|outpoint| {
            let state = match vtxos.iter().find(|vtxo| vtxo.outpoint == *outpoint) {
                None => JournaledInput::Missing,
                Some(vtxo) if vtxo.settled_by.is_some() => JournaledInput::Settled,
                Some(vtxo) if vtxo.is_spent => JournaledInput::Spent(vtxo.ark_txid),
                Some(_) if spendable.contains(outpoint) => JournaledInput::Spendable,
                Some(_) => JournaledInput::Unspendable,
            };
            (*outpoint, state)
        })
        .collect()
}

/// Mark the journaled inputs `spender` took as spent elsewhere when arkd's copy of
/// it, `tx`, pays nothing to the VHTLC `script`. A transaction arkd did not return
/// proves nothing.
pub(super) fn mark_spent_elsewhere(
    inputs: &mut [(OutPoint, JournaledInput)],
    spender: Txid,
    tx: Option<&Psbt>,
    script: &Script,
) {
    if tx.is_some_and(|tx| {
        tx.unsigned_tx
            .output
            .iter()
            .all(|output| output.script_pubkey != *script)
    }) {
        for (_, state) in inputs {
            if *state == JournaledInput::Spent(Some(spender)) {
                *state = JournaledInput::SpentElsewhere;
            }
        }
    }
}

/// The script a payment under `journal` wrote at the VHTLC: ark-core writes a receiver
/// at or above dust as its address's P2TR script. A journal for a different VHTLC
/// address than the swap's current one leaves the funding unresolved, so its inputs
/// never stand in for a payment to this one.
pub(super) fn funding_script(
    journal: Option<&ArkadeFunding>,
    htlc: &ArkadeHtlc,
) -> Result<ScriptBuf> {
    let current = htlc.address();
    let paid = match journal.and_then(|journal| journal.address.as_deref()) {
        Some(address) => ArkAddress::decode(address).map_err(decode)?,
        None => current,
    };
    if paid != current {
        return Err(Error::UnresolvedFunding(
            "a previous funding journaled a different VHTLC address than the swap's; never pay the VHTLC by other means".into(),
        ));
    }
    Ok(paid.to_p2tr_script_pubkey())
}

/// Claims and refunds still accept a deprecated signer, but arkd stops cosigning it
/// at its cutoff, which would strand a payment made to it now.
pub(super) fn validate_funding_signer(htlc: &ArkadeHtlc, info: &Info) -> Result<()> {
    if info.signer_pk.x_only_public_key().0 != htlc.options.server {
        return Err(invalid(
            "the VHTLC's server key is not arkd's current signer; create a new swap",
        ));
    }
    Ok(())
}

/// ark-core writes a receiver below arkd's dust as an OP_RETURN output instead of the
/// VHTLC's script, which the spent-elsewhere proof would read as a payment elsewhere.
pub(super) fn validate_funding_amount(htlc: &ArkadeHtlc, info: &Info) -> Result<()> {
    if htlc.expected_amount < info.dust {
        return Err(invalid(format!(
            "the VHTLC amount {} is below arkd's {} dust limit",
            htlc.expected_amount, info.dust
        )));
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FundingStep {
    Funded(Txid),
    /// Pay from a selection that includes these inputs.
    Pay(Vec<OutPoint>),
    /// The swap no longer awaits funding, and no journaled payment happened.
    NotAwaiting,
}

/// The Ark transaction that may be a journaled payment, preferring one with a VHTLC
/// output, `None` inside when arkd names none of the spenders. Outer `None` when none
/// may be. A payment spends every input of its journal, but a retry that widened its
/// selection while an earlier submission was in flight journaled inputs that
/// submission did not take, so a named spender of any journaled input counts until
/// it is marked spent elsewhere. An unnamed one always counts, since it cannot be
/// fetched.
fn payment_spender(
    inputs: &[(OutPoint, JournaledInput)],
    vhtlc: &[OutPoint],
) -> Option<Option<Txid>> {
    let mut named = inputs.iter().filter_map(|(_, state)| {
        if let JournaledInput::Spent(spender) = *state {
            spender
        } else {
            None
        }
    });
    let paid = named
        .clone()
        .find(|txid| vhtlc.iter().any(|output| output.txid == *txid));
    if let Some(txid) = paid.or_else(|| named.next()) {
        return Some(Some(txid));
    }
    // A spend arkd does not name cannot be fetched, so it may be the payment.
    inputs
        .iter()
        .any(|(_, state)| *state == JournaledInput::Spent(None))
        .then_some(None)
}

/// Decide a funding from its journal and what the server and arkd report now, after
/// the wallet's pending transactions were finalized. `vhtlc` holds every VTXO ever
/// created at the VHTLC address.
pub(super) fn funding_step(
    journaled_txid: Option<Txid>,
    journaled_inputs: &[(OutPoint, JournaledInput)],
    status: &SwapStatus,
    vhtlc: &[OutPoint],
) -> Result<FundingStep> {
    if let Some(txid) = journaled_txid {
        return Ok(FundingStep::Funded(txid));
    }
    // A payment creates a VHTLC output. Inputs left unspent, forfeited into a batch, or
    // spent by transactions paying nothing to the VHTLC prove none happened.
    if let Some(spender) = payment_spender(journaled_inputs, vhtlc) {
        return match spender {
            Some(txid) if vhtlc.iter().any(|output| output.txid == txid) => {
                Ok(FundingStep::Funded(txid))
            }
            Some(_) | None => Err(Error::UnresolvedFunding(UNRESOLVED_FUNDING.into())),
        };
    }
    if journaled_inputs
        .iter()
        .any(|(_, state)| *state == JournaledInput::Missing)
    {
        return Err(Error::UnresolvedFunding(MISSING_INPUT.into()));
    }
    if *status != SwapStatus::Pending {
        return Ok(FundingStep::NotAwaiting);
    }
    // The journal, when the store kept it, proves this client never paid, so outputs it
    // did not journal are refused rather than left unresolved, whoever paid them.
    if !vhtlc.is_empty() {
        return Err(invalid(
            "the VHTLC already holds outputs this client did not journal",
        ));
    }
    // A submission of the journaled payment still in flight spends every journaled
    // input, so a payment sharing the spendable ones conflicts with it in arkd.
    Ok(FundingStep::Pay(
        journaled_inputs
            .iter()
            .filter(|(_, state)| *state == JournaledInput::Spendable)
            .map(|(outpoint, _)| *outpoint)
            .collect(),
    ))
}

/// The refusal that follows [`FundingStep::NotAwaiting`], the one error proving the
/// wallet paid nothing when the store kept the swap's journal.
pub(super) fn not_awaiting(status: &SwapStatus) -> Error {
    invalid(format!("swap is not awaiting funding (status {status:?})"))
}

/// The wallet's VTXOs an offchain payment may draw on, confirmed and pre-confirmed, and
/// what [`select_inputs`] needs to leave out those that expire within the margin.
#[derive(Debug)]
pub(super) struct PaymentCoins {
    pub(super) coins: Vec<Coin>,
    pub(super) now: i64,
    /// From [`expiry_margin`], as offboards use it.
    pub(super) margin: u64,
    /// Recoverable sats, which a refusal for want of balance names.
    pub(super) recoverable: u64,
    pub(super) dust: Amount,
}

impl ArkadeWallet {
    /// The coins an offchain payment from this wallet may spend now.
    pub(super) async fn payment_coins(&self) -> Result<PaymentCoins> {
        let info = self
            .client
            .server_info()
            .await
            .map_err(|e| Error::Transport(format!("server_info: {e}")))?;
        let vtxos = self
            .client
            .list_vtxos()
            .await
            .map_err(|e| Error::Transport(format!("list_vtxos: {e}")))?;
        let now = i64::try_from(unix_now()?).map_err(decode)?;
        Ok(PaymentCoins {
            coins: vtxos
                .spendable_offchain_at(&info, now)
                .map(|entry| Coin {
                    outpoint: entry.vtxo().outpoint,
                    script_pubkey: entry.vtxo().script.clone(),
                    expire_at: entry.vtxo().expires_at,
                    amount: entry.vtxo().amount,
                    assets: entry.vtxo().assets.clone(),
                })
                .collect(),
            now,
            margin: expiry_margin(
                info.session_duration,
                vtxo_lifetime(vtxos.all().map(|entry| entry.vtxo())),
            ),
            recoverable: vtxos
                .recoverable()
                .map(|entry| entry.vtxo().amount.to_sat())
                .fold(0, u64::saturating_add),
            dust: info.dust,
        })
    }
}

fn total(coins: &[Coin]) -> Result<Amount> {
    coins
        .iter()
        .try_fold(Amount::ZERO, |sum, coin| sum.checked_add(coin.amount))
        .ok_or_else(|| invalid("Arkade balance overflow"))
}

/// Inputs paying `amount`: every coin in `required`, topped up from the other coins by
/// ark-core's selection when they fall short. Coins that expire within the margin stay
/// out, as the payment's outputs would expire with them. A balance short without them
/// is refused naming them; a required one fails the call until it has expired, as a
/// payment that left it out would not conflict with one still in flight.
pub(super) fn select_inputs(
    required: &[OutPoint],
    wallet: PaymentCoins,
    amount: Amount,
) -> Result<Vec<OutPoint>> {
    let PaymentCoins {
        coins,
        now,
        margin,
        recoverable,
        dust,
    } = wallet;
    let (kept, others): (Vec<_>, Vec<_>) = coins
        .into_iter()
        .partition(|coin| required.contains(&coin.outpoint));
    // The required inputs were spendable in an earlier read of arkd, so a sweep or
    // spend landed between the two reads, which the same call resolves.
    if kept.len() != required.len() {
        return Err(Error::Transport(
            "an input of a previous VHTLC funding is no longer spendable; retry".into(),
        ));
    }
    let expires_soon = |coin: &Coin| expires_within(coin.expire_at, now, margin);
    if let Some(coin) = kept.iter().find(|coin| expires_soon(coin)) {
        return Err(Error::Transport(format!(
            "a previous VHTLC funding's input of {} sats expires within {margin} s, and a new payment must spend it while that funding may still be in flight; renew it with arkade_renew, or retry once it has expired",
            coin.amount.to_sat()
        )));
    }
    let (expiring, others): (Vec<_>, Vec<_>) =
        others.into_iter().partition(|coin| expires_soon(coin));
    let held = total(&kept)?;
    let spendable = held
        .checked_add(total(&others)?)
        .ok_or_else(|| invalid("Arkade balance overflow"))?;
    if spendable < amount {
        let left_out = LeftOut {
            recoverable,
            expiring: total(&expiring)?.to_sat(),
            margin,
        };
        return Err(invalid(format!(
            "insufficient spendable Arkade balance: {} < {} sats{}",
            spendable.to_sat(),
            amount.to_sat(),
            left_out.hint(dust.to_sat())
        )));
    }
    let mut inputs = required.to_vec();
    match amount.checked_sub(held).filter(|rest| *rest > Amount::ZERO) {
        Some(rest) => inputs.extend(
            select_vtxos(others, rest, dust, true)
                .map_err(|e| invalid(format!("Arkade coin selection: {e}")))?
                .into_iter()
                .map(|coin| coin.outpoint),
        ),
        // As select_vtxos does: sub-dust change could only be recovered in a batch.
        None => {
            let change = held.checked_sub(amount).unwrap_or(Amount::ZERO);
            if change > Amount::ZERO
                && change < dust
                && let Some(coin) = others.iter().min_by_key(|coin| coin.expire_at)
            {
                inputs.push(coin.outpoint);
            }
        }
    }
    Ok(inputs)
}

impl Client {
    /// Pay the VHTLC of an Arkade to Lightning or Arkade to EVM swap from the internal
    /// Arkade wallet. The inputs are journaled before the payment and its txid after
    /// it. Refuses with [`Error::InvalidSwap`] a swap that is not `Pending`, whose
    /// VHTLC holds outputs this client did not journal, or whose VHTLC is locked to a
    /// server key other than arkd's current signer (create a new swap then). Before
    /// journaling, it also refuses a VHTLC amount below arkd's dust, and a VHTLC whose
    /// hash lock is not the swap's: the hash of this client's preimage for Arkade to
    /// EVM, the payment hash of the stored invoice for Arkade to Lightning. A new
    /// selection draws on confirmed and pre-confirmed VTXOs, less those that expire
    /// within the margin [`Client::arkade_offboard`] leaves, since the VHTLC would
    /// expire with them; a balance short without them is refused with
    /// [`Error::InvalidSwap`] naming the sats left out as expiring or recoverable.
    ///
    /// A retry returns a journaled txid. Otherwise an Ark transaction arkd names as the
    /// spender of any journaled input may be the payment, or that of an earlier attempt
    /// still in flight when a retry widened the selection: this returns that
    /// transaction once its VHTLC output is visible and fails with
    /// [`Error::UnresolvedFunding`] until then, unless arkd returns it and none of its
    /// outputs pays the VHTLC. A journaled input spent without arkd naming a spender, a
    /// journaled input arkd does not know, and a journal for a VHTLC address other than
    /// the swap's fail the same way and are never resolved automatically. Inputs left
    /// unspent, forfeited into a batch, or spent by transactions that pay nothing to the
    /// VHTLC prove the payment never happened. The swap is then paid again under the
    /// checks above from a selection that includes every journaled input still
    /// spendable, so arkd accepts at most one of the two payments if the first is still
    /// in flight. When no journaled input is spendable any more, that rests on arkd also
    /// refusing the ones this client sees as expired, swept, unrolled or under a retired
    /// signer. A swap that no longer awaits funding instead has such a journal emptied
    /// before the refusal, which ends a tracker's wait on its recovery. With a store that
    /// kept this swap's journal, that refusal is the only error proving the wallet paid
    /// nothing: the others, such as a refusal before the journal is read, leave an
    /// earlier payment possible. A swap recovered on another device has no journal, and
    /// an index-only store keeps it only for one `Client`'s life.
    ///
    /// [`Error::Transport`], or [`Error::Timeout`] after 120 seconds, leaves the outcome
    /// unknown: call this method again, which resolves it from the journal, and never
    /// pay the VHTLC by other means. The 120 seconds start once any wallet spend in
    /// progress has finished. Once arkd accepted the payment, a failure to journal its
    /// txid is logged and the txid returned; a retry resolves it from the journaled
    /// inputs. A journaled input that the new payment must spend but that expires
    /// within the margin also fails with [`Error::Transport`], until
    /// [`Client::arkade_renew`] renews it or it has expired.
    pub async fn fund_arkade_swap(&self, swap_id: &str) -> Result<Txid> {
        self.wallet_spend(
            crate::client::WALLET_PAYMENT_TIMEOUT,
            Box::pin(self.fund_from_wallet(swap_id)),
        )
        .await
    }

    async fn fund_from_wallet(&self, swap_id: &str) -> Result<Txid> {
        if self.get_stored_swap(swap_id)?.is_none() {
            self.refresh_swap(swap_id).await?;
        }
        let htlc = self.arkade_htlc(swap_id)?;
        if htlc.is_receiver {
            return Err(invalid(
                "only Arkade to Lightning and Arkade to EVM swaps are funded from the wallet",
            ));
        }
        let current = self.get_swap_details(swap_id).await?;
        let (address, amount) = match &current {
            GetSwapResponse::ArkadeToLightning(r) => (&r.arkade_vhtlc_address, &r.source_amount),
            GetSwapResponse::ArkadeToEvm(r) => (&r.btc_vhtlc_address, &r.source_amount),
            _ => return Err(invalid("server swap details differ from the stored swap")),
        };
        if *address != htlc.address().encode()
            || amount.parse::<u64>().map_err(decode)? != htlc.expected_amount.to_sat()
        {
            return Err(invalid("server swap details differ from the stored swap"));
        }
        let wallet = self.arkade_wallet().await?;
        // An interrupted payment becomes visible once arkd finalizes it.
        wallet
            .client
            .continue_pending_offchain_txs()
            .await
            .map_err(|e| Error::Transport(format!("continue_pending_offchain_txs: {e}")))?;
        let info = wallet.info().await?;
        validate_info(&htlc, &info)?;
        let vhtlc: Vec<_> = validate_vtxos(&htlc, wallet.vtxos(htlc.address()).await?)?
            .iter()
            .map(|vtxo| vtxo.outpoint)
            .collect();
        let stored = self
            .get_stored_swap(swap_id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        let journal = stored.metadata.arkade_funding.clone();
        let script = funding_script(journal.as_ref(), &htlc)?;
        let txid = journal
            .as_ref()
            .and_then(|journal| journal.txid.as_deref())
            .map(Txid::from_str)
            .transpose()
            .map_err(decode)?;
        let journaled = journal
            .iter()
            .flat_map(|journal| &journal.inputs)
            .map(|outpoint| OutPoint::from_str(outpoint).map_err(decode))
            .collect::<Result<Vec<_>>>()?;
        let now = i64::try_from(unix_now()?).map_err(decode)?;
        let inputs = if journaled.is_empty() {
            Vec::new()
        } else {
            let list = wallet
                .client
                .list_vtxos_for_outpoints(journaled.clone())
                .await
                .map_err(|e| Error::Transport(format!("list_vtxos_for_outpoints: {e}")))?;
            let spendable: HashSet<_> = list
                .spendable_offchain_at(&info, now)
                .map(|entry| entry.vtxo().outpoint)
                .collect();
            let vtxos: Vec<_> = list.all().map(|entry| entry.vtxo().clone()).collect();
            let mut inputs = classify_inputs(&journaled, &vtxos, &spendable);
            let mut fetched = HashSet::new();
            while txid.is_none()
                && let Some(Some(spender)) = payment_spender(&inputs, &vhtlc)
                && !vhtlc.iter().any(|output| output.txid == spender)
                && fetched.insert(spender)
            {
                let tx = wallet.virtual_transaction(spender).await?;
                mark_spent_elsewhere(&mut inputs, spender, tx.as_ref(), &script);
            }
            inputs
        };
        let required = match funding_step(txid, &inputs, current.status(), &vhtlc)? {
            FundingStep::Funded(txid) => {
                if let Some(journal) = journal.filter(|journal| journal.txid.is_none()) {
                    self.journal_funding_txid(swap_id, journal, txid);
                }
                return Ok(txid);
            }
            FundingStep::Pay(required) => required,
            FundingStep::NotAwaiting => {
                self.clear_unpaid_funding(swap_id, journal)?;
                return Err(not_awaiting(current.status()));
            }
        };
        self.validate_funding_hash_lock(&htlc, &stored)?;
        validate_funding_signer(&htlc, &info)?;
        validate_funding_amount(&htlc, &info)?;
        let inputs = select_inputs(
            &required,
            wallet.payment_coins().await?,
            htlc.expected_amount,
        )?;
        let journal = ArkadeFunding {
            inputs: inputs.iter().map(ToString::to_string).collect(),
            address: Some(htlc.address().encode()),
            txid: None,
        };
        self.save_arkade_funding(swap_id, journal.clone())?;
        let txid = wallet
            .client
            .send_selection(
                &inputs,
                vec![SendReceiver::bitcoin(htlc.address(), htlc.expected_amount)],
            )
            .await
            .map_err(|e| Error::Transport(format!("fund_arkade_swap: {e}")))?;
        self.journal_funding_txid(swap_id, journal, txid);
        Ok(txid)
    }

    /// A VHTLC under any other hash than the swap's is claimed by whoever chose that
    /// hash, without the server's side of the swap: for Arkade to EVM the hash of this
    /// client's preimage, for Arkade to Lightning the payment hash of the stored
    /// invoice. This SDK checks an invoice before storing it, but one an older SDK
    /// stored may be unchecked.
    pub(super) fn validate_funding_hash_lock(
        &self,
        htlc: &ArkadeHtlc,
        stored: &crate::StoredSwap,
    ) -> Result<()> {
        let (hash, refusal) = match &stored.response {
            GetSwapResponse::ArkadeToEvm(_) => (
                self.signer
                    .as_ref()
                    .ok_or_else(|| invalid("signer missing"))?
                    .derive_swap_params(stored.key_index)?
                    .hash_lock,
                "swap hash lock does not match the seed-derived preimage",
            ),
            GetSwapResponse::ArkadeToLightning(r) => (
                crate::client::invoice_payment_hash(&r.client_lightning_invoice)?,
                crate::client::INVOICE_HASH_MISMATCH,
            ),
            _ => return Err(invalid("swap has no Arkade HTLC the client funds")),
        };
        if htlc.options.preimage_hash != ripemd160::Hash::hash(&hash) {
            return Err(invalid(refusal));
        }
        Ok(())
    }

    /// The payment went out whether or not the write succeeds, and a retry re-derives
    /// the txid from the spent journaled inputs, so a failure is only logged.
    fn journal_funding_txid(&self, swap_id: &str, mut journal: ArkadeFunding, txid: Txid) {
        journal.txid = Some(txid.to_string());
        if let Err(error) = self.save_arkade_funding(swap_id, journal) {
            sdk_log!(
                self.log.child("arkade", "arkade.fund").swap_id(swap_id),
                Warn,
                "arkade.fund.txidNotJournaled",
                "VHTLC funding went out but its txid was not journaled; a retry resolves it from the inputs",
                "arkTxid" => txid.to_string();
                error = error
            );
        }
    }

    /// Empty a journal shown never paid, which ends a tracker's wait on its recovery.
    /// Writing `None` would not, since metadata merges skip it.
    pub(crate) fn clear_unpaid_funding(
        &self,
        swap_id: &str,
        journal: Option<ArkadeFunding>,
    ) -> Result<()> {
        match journal {
            Some(journal) if !journal.inputs.is_empty() => self.save_arkade_funding(
                swap_id,
                ArkadeFunding {
                    inputs: Vec::new(),
                    address: None,
                    txid: None,
                },
            ),
            Some(_) | None => Ok(()),
        }
    }

    fn save_arkade_funding(&self, id: &str, funding: ArkadeFunding) -> Result<()> {
        self.update_swap_metadata(
            id,
            SwapMetadata {
                arkade_funding: Some(funding),
                ..Default::default()
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteSwapStorage;
    use crate::StoredSwap;
    use crate::SwapStorage;

    fn outpoint(byte: u8, vout: u32) -> OutPoint {
        OutPoint::new(Txid::from_byte_array([byte; 32]), vout)
    }

    fn txid(byte: u8) -> Txid {
        Txid::from_byte_array([byte; 32])
    }

    fn is_unresolved(error: &Error, text: &str) -> bool {
        matches!(error, Error::UnresolvedFunding(message) if message == text)
    }

    fn refused_as_not_journaled(step: Result<FundingStep>) -> bool {
        matches!(step, Err(Error::InvalidSwap(message)) if message.contains("did not journal"))
    }

    #[test]
    fn a_journaled_txid_is_returned_whatever_the_swap_shows() {
        let spent = [(outpoint(1, 0), JournaledInput::Spent(None))];
        for status in [SwapStatus::Pending, SwapStatus::ServerRedeemed] {
            assert_eq!(
                funding_step(Some(txid(9)), &spent, &status, &[outpoint(8, 0)]).unwrap(),
                FundingStep::Funded(txid(9))
            );
        }
    }

    #[test]
    fn spent_inputs_are_funded_only_when_their_payment_reached_the_vhtlc() {
        let spent = [
            (outpoint(1, 0), JournaledInput::Spent(Some(txid(7)))),
            (outpoint(1, 1), JournaledInput::Spent(Some(txid(7)))),
        ];
        let partly_named = [
            (outpoint(1, 0), JournaledInput::Spent(None)),
            (outpoint(1, 1), JournaledInput::Spent(Some(txid(7)))),
        ];
        for inputs in [&spent[..], &partly_named[..]] {
            let funded = funding_step(None, inputs, &SwapStatus::Pending, &[outpoint(7, 0)]);
            assert_eq!(funded.unwrap(), FundingStep::Funded(txid(7)));
        }

        // Not visible yet, a VHTLC output of another payment, or a spender arkd does
        // not name: never pay, whatever the swap shows.
        let unnamed = [(outpoint(1, 0), JournaledInput::Spent(None))];
        let unnamed_beside_unspent = [
            (outpoint(1, 0), JournaledInput::Spent(None)),
            (outpoint(1, 1), JournaledInput::Spendable),
        ];
        for (inputs, vhtlc) in [
            (&spent[..], &[][..]),
            (&spent[..], &[outpoint(5, 0)][..]),
            (&unnamed[..], &[outpoint(7, 0)][..]),
            (&unnamed_beside_unspent[..], &[][..]),
        ] {
            for status in [SwapStatus::Pending, SwapStatus::ClientFunded] {
                let error = funding_step(None, inputs, &status, vhtlc).unwrap_err();
                assert!(is_unresolved(&error, UNRESOLVED_FUNDING), "{error}");
            }
        }
    }

    #[test]
    fn inputs_no_payment_can_have_spent_prove_the_payment_never_happened() {
        let partly = [
            (outpoint(1, 0), JournaledInput::SpentElsewhere),
            (outpoint(1, 1), JournaledInput::Spendable),
        ];
        let batch = [(outpoint(1, 0), JournaledInput::Settled)];
        let batch_and_spent = [
            (outpoint(1, 0), JournaledInput::Settled),
            (outpoint(1, 1), JournaledInput::SpentElsewhere),
        ];
        for (inputs, keep) in [
            (&partly[..], vec![outpoint(1, 1)]),
            (&batch[..], vec![]),
            (&batch_and_spent[..], vec![]),
        ] {
            assert_eq!(
                funding_step(None, inputs, &SwapStatus::Pending, &[]).unwrap(),
                FundingStep::Pay(keep)
            );
            let held = funding_step(None, inputs, &SwapStatus::Pending, &[outpoint(7, 0)]);
            assert!(refused_as_not_journaled(held));
            assert_eq!(
                funding_step(None, inputs, &SwapStatus::ClientFunded, &[]).unwrap(),
                FundingStep::NotAwaiting
            );
        }
    }

    /// An Ark transaction paying `script` and an unrelated output.
    fn payment(script: &bitcoin::ScriptBuf) -> Psbt {
        Psbt::from_unsigned_tx(bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![],
            output: vec![
                bitcoin::TxOut {
                    value: Amount::from_sat(1_000),
                    script_pubkey: bitcoin::ScriptBuf::from_bytes(vec![0x52]),
                },
                bitcoin::TxOut {
                    value: Amount::from_sat(1_000),
                    script_pubkey: script.clone(),
                },
            ],
        })
        .unwrap()
    }

    #[test]
    fn a_spender_paying_nothing_to_the_vhtlc_proves_the_payment_never_happened() {
        let vhtlc = bitcoin::ScriptBuf::from_bytes(vec![0x51]);
        let elsewhere = payment(&bitcoin::ScriptBuf::from_bytes(vec![0x53]));
        let marked = |mut inputs: Vec<(OutPoint, JournaledInput)>, tx: Option<&Psbt>| {
            mark_spent_elsewhere(&mut inputs, txid(7), tx, &vhtlc);
            inputs
        };
        let spent = vec![
            (outpoint(1, 0), JournaledInput::Spent(Some(txid(7)))),
            (outpoint(1, 1), JournaledInput::Spent(Some(txid(7)))),
        ];
        let partly_named = vec![
            (outpoint(1, 0), JournaledInput::Spent(None)),
            (outpoint(1, 1), JournaledInput::Spent(Some(txid(7)))),
        ];
        assert_eq!(
            marked(spent.clone(), Some(&elsewhere)),
            vec![
                (outpoint(1, 0), JournaledInput::SpentElsewhere),
                (outpoint(1, 1), JournaledInput::SpentElsewhere),
            ]
        );
        // The unnamed spender beside it cannot be fetched, so it may still be the payment.
        let unresolved = marked(partly_named, Some(&elsewhere));
        let error = funding_step(None, &unresolved, &SwapStatus::Pending, &[]).unwrap_err();
        assert!(is_unresolved(&error, UNRESOLVED_FUNDING), "{error}");
        {
            let inputs = marked(spent.clone(), Some(&elsewhere));
            assert_eq!(
                funding_step(None, &inputs, &SwapStatus::Pending, &[]).unwrap(),
                FundingStep::Pay(vec![])
            );
            assert_eq!(
                funding_step(None, &inputs, &SwapStatus::Expired, &[]).unwrap(),
                FundingStep::NotAwaiting
            );
            let held = funding_step(None, &inputs, &SwapStatus::Pending, &[outpoint(5, 0)]);
            assert!(refused_as_not_journaled(held));
        }
        // A spender that pays the VHTLC, or one arkd did not return, may still be the
        // payment.
        for tx in [Some(&payment(&vhtlc)), None] {
            let inputs = marked(spent.clone(), tx);
            assert_eq!(inputs, spent);
            let error = funding_step(None, &inputs, &SwapStatus::Pending, &[]).unwrap_err();
            assert!(is_unresolved(&error, UNRESOLVED_FUNDING), "{error}");
        }
        let other = vec![(outpoint(1, 0), JournaledInput::Spent(Some(txid(6))))];
        assert_eq!(marked(other.clone(), Some(&elsewhere)), other);
        let missing = marked(
            vec![
                (outpoint(1, 0), JournaledInput::Spent(Some(txid(7)))),
                (outpoint(1, 1), JournaledInput::Missing),
            ],
            Some(&elsewhere),
        );
        let error = funding_step(None, &missing, &SwapStatus::Pending, &[]).unwrap_err();
        assert!(is_unresolved(&error, MISSING_INPUT), "{error}");
    }

    #[test]
    fn a_widened_retry_never_pays_beside_a_spender_that_pays_the_vhtlc() {
        let vhtlc = bitcoin::ScriptBuf::from_bytes(vec![0x51]);
        // The first attempt journaled (1, 0) alone, and its payment was still in flight
        // when a retry journaled (1, 0) and (1, 1).
        let widened = vec![
            (outpoint(1, 0), JournaledInput::Spent(Some(txid(7)))),
            (outpoint(1, 1), JournaledInput::Spendable),
        ];
        let marked = |tx: &Psbt| {
            let mut inputs = widened.clone();
            mark_spent_elsewhere(&mut inputs, txid(7), Some(tx), &vhtlc);
            inputs
        };
        let paid = marked(&payment(&vhtlc));
        for status in [SwapStatus::Pending, SwapStatus::ClientFunded] {
            let error = funding_step(None, &paid, &status, &[]).unwrap_err();
            assert!(is_unresolved(&error, UNRESOLVED_FUNDING), "{error}");
        }
        assert_eq!(
            funding_step(None, &paid, &SwapStatus::Pending, &[outpoint(7, 0)]).unwrap(),
            FundingStep::Funded(txid(7))
        );
        let elsewhere = marked(&payment(&bitcoin::ScriptBuf::from_bytes(vec![0x53])));
        assert_eq!(
            funding_step(None, &elsewhere, &SwapStatus::Pending, &[]).unwrap(),
            FundingStep::Pay(vec![outpoint(1, 1)])
        );
        // Each spender not marked elsewhere may be a payment, a visible one first.
        let split = [
            (outpoint(1, 0), JournaledInput::Spent(Some(txid(6)))),
            (outpoint(1, 1), JournaledInput::Spent(Some(txid(7)))),
        ];
        let error = funding_step(None, &split, &SwapStatus::Pending, &[]).unwrap_err();
        assert!(is_unresolved(&error, UNRESOLVED_FUNDING), "{error}");
        assert_eq!(
            funding_step(None, &split, &SwapStatus::Pending, &[outpoint(7, 0)]).unwrap(),
            FundingStep::Funded(txid(7))
        );
    }

    #[test]
    fn a_txid_that_cannot_be_journaled_is_logged_instead_of_failing_the_payment() {
        let mut client = Client::builder()
            .mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about")
            .build()
            .unwrap();
        let logger = std::sync::Arc::new(crate::logging::RecordingLogger::default());
        client.set_logger(logger.clone(), crate::logging::LogLevel::Trace);
        let journal = ArkadeFunding {
            inputs: vec![outpoint(1, 0).to_string()],
            address: None,
            txid: None,
        };
        client.journal_funding_txid("missing", journal, txid(7));
        let record = logger.find("arkade.fund.txidNotJournaled").unwrap();
        assert_eq!(record.data["arkTxid"], txid(7).to_string());
        assert!(record.error.unwrap().contains("swap record not found"));
    }

    #[test]
    fn a_new_payment_keeps_every_journaled_input_that_is_still_spendable() {
        let unspent = [
            (outpoint(1, 0), JournaledInput::Spendable),
            (outpoint(1, 1), JournaledInput::Spendable),
        ];
        assert_eq!(
            funding_step(None, &unspent, &SwapStatus::Pending, &[]).unwrap(),
            FundingStep::Pay(vec![outpoint(1, 0), outpoint(1, 1)])
        );
        let expired = [
            (outpoint(1, 0), JournaledInput::Unspendable),
            (outpoint(1, 1), JournaledInput::Spendable),
        ];
        assert_eq!(
            funding_step(None, &expired, &SwapStatus::Pending, &[]).unwrap(),
            FundingStep::Pay(vec![outpoint(1, 1)])
        );
        let gone = [(outpoint(1, 0), JournaledInput::Unspendable)];
        assert_eq!(
            funding_step(None, &gone, &SwapStatus::Pending, &[]).unwrap(),
            FundingStep::Pay(vec![])
        );
        let missing = [
            (outpoint(1, 0), JournaledInput::Missing),
            (outpoint(1, 1), JournaledInput::Spendable),
        ];
        let error = funding_step(None, &missing, &SwapStatus::Pending, &[]).unwrap_err();
        assert!(is_unresolved(&error, MISSING_INPUT), "{error}");
    }

    fn vtxo(outpoint: OutPoint) -> VirtualTxOutPoint {
        VirtualTxOutPoint {
            outpoint,
            created_at: 0,
            expires_at: i64::MAX,
            amount: Amount::from_sat(10_000),
            script: bitcoin::ScriptBuf::new(),
            is_preconfirmed: false,
            is_swept: false,
            is_unrolled: false,
            is_spent: false,
            spent_by: None,
            commitment_txids: vec![],
            settled_by: None,
            ark_txid: None,
            assets: vec![],
            depth: 0,
        }
    }

    #[test]
    fn journaled_inputs_are_classified_from_arkds_view() {
        let mut offchain = vtxo(outpoint(1, 0));
        offchain.is_spent = true;
        offchain.spent_by = Some(txid(3));
        offchain.ark_txid = Some(txid(7));
        let mut unnamed = vtxo(outpoint(1, 1));
        unnamed.is_spent = true;
        unnamed.spent_by = Some(txid(3));
        // Batch detection must not depend on the Ark txid being absent.
        let mut forfeited = vtxo(outpoint(1, 2));
        forfeited.is_spent = true;
        forfeited.settled_by = Some(txid(8));
        forfeited.ark_txid = Some(txid(7));
        let mut unrolled = vtxo(outpoint(1, 3));
        unrolled.is_unrolled = true;
        let mut expired = vtxo(outpoint(1, 4));
        expired.expires_at = 1;
        let mut swept = vtxo(outpoint(1, 5));
        swept.is_swept = true;
        let vtxos = [
            offchain,
            unnamed,
            forfeited,
            unrolled,
            expired,
            swept,
            vtxo(outpoint(1, 6)),
        ];
        // The predicate behind the wallet's spendable_offchain_at, short of retired
        // signers.
        let spendable: HashSet<_> = VtxoList::new(Amount::from_sat(330), vtxos.to_vec())
            .spendable_offchain()
            .map(|vtxo| vtxo.outpoint)
            .collect();
        let journaled: Vec<_> = (0..8).map(|vout| outpoint(1, vout)).collect();
        assert_eq!(
            classify_inputs(&journaled, &vtxos, &spendable),
            vec![
                (outpoint(1, 0), JournaledInput::Spent(Some(txid(7)))),
                (outpoint(1, 1), JournaledInput::Spent(None)),
                (outpoint(1, 2), JournaledInput::Settled),
                (outpoint(1, 3), JournaledInput::Unspendable),
                (outpoint(1, 4), JournaledInput::Unspendable),
                (outpoint(1, 5), JournaledInput::Unspendable),
                (outpoint(1, 6), JournaledInput::Spendable),
                (outpoint(1, 7), JournaledInput::Missing),
            ]
        );
        // A healthy VTXO under a retired signer is missing from the spendable set.
        assert_eq!(
            classify_inputs(&[outpoint(1, 6)], &vtxos, &HashSet::new()),
            vec![(outpoint(1, 6), JournaledInput::Unspendable)]
        );
    }

    fn coin(vout: u32, sats: u64, expire_at: i64) -> Coin {
        Coin {
            outpoint: outpoint(2, vout),
            script_pubkey: bitcoin::ScriptBuf::new(),
            expire_at,
            amount: Amount::from_sat(sats),
            assets: vec![],
        }
    }

    fn payment_coins(coins: Vec<Coin>, now: i64, margin: u64) -> PaymentCoins {
        PaymentCoins {
            coins,
            now,
            margin,
            recoverable: 0,
            dust: Amount::from_sat(330),
        }
    }

    #[test]
    fn a_new_selection_keeps_the_required_inputs_and_tops_them_up() {
        let coins = || {
            vec![
                coin(0, 40_000, 10),
                coin(1, 30_000, 20),
                coin(2, 50_000, 30),
            ]
        };
        let select = |required: &[OutPoint], sats| {
            select_inputs(
                required,
                payment_coins(coins(), 0, 0),
                Amount::from_sat(sats),
            )
        };
        assert_eq!(
            select(&[outpoint(2, 2)], 50_000).unwrap(),
            vec![outpoint(2, 2)]
        );
        // Short required inputs are topped up with the coins expiring first.
        assert_eq!(
            select(&[outpoint(2, 2)], 80_000).unwrap(),
            vec![outpoint(2, 2), outpoint(2, 0)]
        );
        assert_eq!(
            select(&[], 60_000).unwrap(),
            vec![outpoint(2, 0), outpoint(2, 1)]
        );
        // Required inputs leaving sub-dust change take the coin expiring first, as
        // ark-core's selection does.
        assert_eq!(
            select(&[outpoint(2, 2)], 49_900).unwrap(),
            vec![outpoint(2, 2), outpoint(2, 0)]
        );
        assert_eq!(
            select(&[outpoint(2, 2)], 49_600).unwrap(),
            vec![outpoint(2, 2)]
        );
        // A required input the wallet can no longer spend is never replaced.
        assert!(matches!(
            select(&[outpoint(9, 0)], 1_000),
            Err(Error::Transport(_))
        ));
    }

    #[test]
    fn a_send_or_funding_from_only_expiring_vtxos_is_refused_naming_them() {
        // The VTXO that paid a regtest VHTLC 6 s before it expired.
        let now = 1_791_718_556;
        let margin = expiry_margin(30, Some(512));
        let refusal = |recoverable| {
            let wallet = PaymentCoins {
                recoverable,
                ..payment_coins(vec![coin(0, 15_045, now + 6)], now, margin)
            };
            match select_inputs(&[], wallet, Amount::from_sat(15_045)) {
                Err(Error::InvalidSwap(message)) => message,
                other => panic!("expected a refusal, got {other:?}"),
            }
        };
        assert_eq!(
            refusal(0),
            "insufficient spendable Arkade balance: 0 < 15045 sats; 15045 sats expire within 256 s and can be renewed with arkade_renew, or settled with arkade_settle once expired"
        );
        let message = refusal(1_000);
        assert!(
            message.contains("15045 sats expire within 256 s")
                && message.contains("settle the 1000 recoverable sats with arkade_settle first"),
            "{message}"
        );
    }

    #[test]
    fn a_send_or_funding_spends_only_vtxos_outside_the_margin() {
        let now = 2_000_000_000;
        let margin = expiry_margin(30, Some(512));
        let coins = || {
            vec![
                coin(0, 20_000, now + 6),
                // Each side of the margin, where offboards draw it too.
                coin(1, 30_000, now + 256),
                coin(2, 50_000, now + 257),
                coin(3, 40_000, now + 10_000),
            ]
        };
        let select = |required: &[OutPoint], sats| {
            select_inputs(
                required,
                payment_coins(coins(), now, margin),
                Amount::from_sat(sats),
            )
        };
        // ark-core's selection alone would take the two soonest-expiring coins.
        assert_eq!(select(&[], 45_000).unwrap(), vec![outpoint(2, 2)]);
        assert_eq!(
            select(&[], 90_000).unwrap(),
            vec![outpoint(2, 2), outpoint(2, 3)]
        );
        assert_eq!(
            select(&[outpoint(2, 3)], 60_000).unwrap(),
            vec![outpoint(2, 3), outpoint(2, 2)]
        );
        // Sub-dust change takes the soonest-expiring coin outside the margin.
        assert_eq!(
            select(&[outpoint(2, 3)], 39_900).unwrap(),
            vec![outpoint(2, 3), outpoint(2, 2)]
        );
        let message = select(&[], 90_001).unwrap_err().to_string();
        assert!(
            message.contains("90000 < 90001 sats; 50000 sats expire within 256 s"),
            "{message}"
        );
    }

    #[test]
    fn a_retry_fails_while_a_journaled_input_expires_within_the_margin() {
        let now = 2_000_000_000;
        let margin = expiry_margin(30, Some(512));
        let coins = vec![coin(0, 20_000, now + 200), coin(1, 50_000, now + 10_000)];
        // Paying from the other coin alone would not conflict with a payment of the
        // journaled one still in flight.
        let error = select_inputs(
            &[outpoint(2, 0)],
            payment_coins(coins, now, margin),
            Amount::from_sat(10_000),
        )
        .unwrap_err();
        assert!(
            matches!(&error, Error::Transport(message) if message.contains("input of 20000 sats expires within 256 s")),
            "{error:?}"
        );
    }

    #[test]
    fn without_a_payment_of_ours_only_an_empty_pending_vhtlc_is_paid() {
        let unspent = [(outpoint(1, 0), JournaledInput::Spendable)];
        for inputs in [&[][..], &unspent[..]] {
            let funded = funding_step(None, inputs, &SwapStatus::Pending, &[outpoint(5, 0)]);
            assert!(refused_as_not_journaled(funded));
            for status in [SwapStatus::Expired, SwapStatus::ClientFunded] {
                assert_eq!(
                    funding_step(None, inputs, &status, &[]).unwrap(),
                    FundingStep::NotAwaiting
                );
            }
        }
        assert_eq!(
            funding_step(None, &[], &SwapStatus::Pending, &[]).unwrap(),
            FundingStep::Pay(vec![])
        );
        // Hosts take this refusal alone as proof the wallet paid nothing.
        assert!(matches!(
            not_awaiting(&SwapStatus::Expired),
            Error::InvalidSwap(message) if message == "swap is not awaiting funding (status Expired)"
        ));
    }

    #[test]
    fn the_funding_journal_survives_storage_merges_and_reopening() {
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
        let record = StoredSwap::new(
            0,
            serde_json::from_value(fixtures["arkade_to_lightning"].clone()).unwrap(),
            SwapMetadata::default(),
        )
        .unwrap();
        let id = record.response.id().to_owned();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("swaps.db");
        let store = SqliteSwapStorage::open(&path, "wallet").unwrap();
        store.store_swap(record).unwrap();
        let journal = ArkadeFunding {
            inputs: vec![outpoint(1, 0).to_string()],
            address: Some("tark1vhtlc".into()),
            txid: None,
        };
        let update = |metadata| store.update_metadata(&id, metadata).unwrap();
        update(SwapMetadata {
            arkade_funding: Some(journal.clone()),
            ..Default::default()
        });
        update(SwapMetadata {
            target_address: Some("unrelated".into()),
            ..Default::default()
        });
        let stored = SqliteSwapStorage::open(&path, "wallet")
            .unwrap()
            .get_swap(&id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.metadata.arkade_funding, Some(journal));
        assert!(stored.metadata.extra.is_empty());
    }
}
