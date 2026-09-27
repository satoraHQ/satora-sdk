use super::*;
use crate::bitcoin::BitcoinBackend;
use crate::bitcoin::BitcoinHistory;
use bitcoin::OutPoint;
use bitcoin::Script;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::taproot::ControlBlock;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

pub struct BitcoinReader {
    backend: Arc<dyn BitcoinBackend>,
}
impl BitcoinReader {
    pub fn new(backend: Arc<dyn BitcoinBackend>) -> Self {
        Self { backend }
    }
}
impl ContractReader for BitcoinReader {
    fn can_observe(&self, reference: &ContractRef) -> bool {
        matches!(reference,ContractRef::Bitcoin {htlc,..} if htlc.network==self.backend.network())
    }
    fn observe<'a>(&'a self, reference: &'a ContractRef) -> ObservationFuture<'a, Observation> {
        Box::pin(async move {
            let ContractRef::Bitcoin {
                htlc,
                expected_sats,
                confirmations,
            } = reference
            else {
                return Err(invalid("Bitcoin reader received another ledger"));
            };
            if !self.can_observe(reference) {
                return Err(invalid("Bitcoin reader network mismatch"));
            }
            let history = self.backend.history(&htlc.address()?).await?;
            classify(htlc, *expected_sats, *confirmations, &history)
        })
    }
}
fn confirmed(height: Option<u32>, tip: u32, required: u32) -> bool {
    required == 0 || height.is_some_and(|height| height <= tip && tip - height + 1 >= required)
}
pub(super) fn classify(
    htlc: &crate::bitcoin::BitcoinHtlc,
    expected: u64,
    confirmations: u32,
    history: &BitcoinHistory,
) -> Result<Observation> {
    if expected == 0 {
        return Err(invalid("expected Bitcoin funding must be positive"));
    }
    let point = &history.observation;
    let scripts = htlc.scripts()?;
    let script = htlc.address()?.script_pubkey();
    let mut funding = HashMap::new();
    let mut spends = HashMap::new();
    let mut txids = HashSet::new();
    for entry in &history.transactions {
        let txid = entry.transaction.compute_txid();
        if !txids.insert(txid) {
            return Err(invalid("duplicate Bitcoin history transaction"));
        }
        if entry
            .confirmed_height
            .is_some_and(|height| height > point.height)
        {
            return Err(invalid("Bitcoin history height exceeds tip"));
        }
        for (index, output) in entry
            .transaction
            .output
            .iter()
            .enumerate()
            .filter(|(_, output)| output.script_pubkey == script)
        {
            funding.insert(
                OutPoint {
                    txid,
                    vout: u32::try_from(index).map_err(|e| Error::Decode(e.to_string()))?,
                },
                (output.value.to_sat(), entry.confirmed_height),
            );
        }
        for input in &entry.transaction.input {
            if spends
                .insert(input.previous_output, (input, entry.confirmed_height))
                .is_some()
                && !input.previous_output.is_null()
            {
                return Err(invalid("conflicting Bitcoin spends in history"));
            }
        }
    }
    for utxo in &point.utxos {
        if utxo.output.script_pubkey != script
            || funding.get(&utxo.outpoint).is_none_or(|(amount, height)| {
                *amount != utxo.output.value.to_sat() || *height != utxo.confirmed_height
            })
        {
            return Err(invalid("Bitcoin history and unspent outputs disagree"));
        }
    }
    let unspent: HashSet<_> = point.utxos.iter().map(|utxo| utxo.outpoint).collect();
    let (mut total, mut remaining, mut ready) = (0u64, 0u64, 0u64);
    let (mut claims, mut refunds, mut unknown, mut pending) = (0, 0, 0, false);
    let mut preimage = None;
    for (outpoint, (amount, height)) in &funding {
        total = total
            .checked_add(*amount)
            .ok_or_else(|| invalid("Bitcoin funding amount overflow"))?;
        if confirmed(*height, point.height, confirmations) {
            ready = ready
                .checked_add(*amount)
                .ok_or_else(|| invalid("Bitcoin amount overflow"))?;
        }
        if unspent.contains(outpoint) {
            if spends.contains_key(outpoint) {
                return Err(invalid("Bitcoin output is both spent and unspent"));
            }
            remaining = remaining
                .checked_add(*amount)
                .ok_or_else(|| invalid("Bitcoin amount overflow"))?;
            continue;
        }
        let Some((input, spend_height)) = spends.get(outpoint) else {
            unknown += 1;
            continue;
        };
        if !confirmed(*spend_height, point.height, confirmations) {
            pending = true;
        }
        let mut witness: Vec<_> = input.witness.iter().collect();
        if witness
            .last()
            .is_some_and(|bytes| bytes.first() == Some(&0x50))
        {
            witness.pop();
        }
        if witness.len() < 2 {
            unknown += 1;
            continue;
        }
        let control = witness.pop().unwrap();
        let spend_script = Script::from_bytes(witness.pop().unwrap());
        let valid = ControlBlock::decode(control).is_ok_and(|control| {
            control.verify_taproot_commitment(
                &Secp256k1::verification_only(),
                scripts.spend_info.output_key().to_x_only_public_key(),
                spend_script,
            )
        });
        if !valid {
            unknown += 1;
            continue;
        }
        if spend_script == scripts.claim.as_script() {
            if let Some(secret) = witness
                .iter()
                .find_map(|bytes| HashLock::Hash160(htlc.hash_lock).preimage(bytes))
            {
                preimage = Some(secret);
                claims += 1;
            } else {
                unknown += 1;
            }
        } else if spend_script == scripts.refund.as_script() {
            refunds += 1;
        } else {
            unknown += 1;
        }
    }
    let state = if total == 0 {
        HtlcState::Absent
    } else if claims + refunds + unknown > 0 {
        // Anyone can pay the address but only the swap parties can spend it, so
        // outputs left after a spend are surplus and must not revive the leg.
        if pending || unknown > 0 || (claims > 0 && refunds > 0) {
            HtlcState::SpentUnknown
        } else if claims > 0 {
            HtlcState::SpentClaim
        } else {
            HtlcState::SpentRefund
        }
    } else if ready < expected && total >= expected {
        HtlcState::Mempool
    } else if total < expected {
        HtlcState::Invalid
    } else {
        HtlcState::Confirmed
    };
    Ok(Observation {
        state,
        point: Some(ChainPoint {
            height: u64::from(point.height),
            hash: point.tip.to_string(),
            timestamp: u64::from(point.median_time_past),
        }),
        funded_amount: total.to_string(),
        remaining_amount: remaining.to_string(),
        preimage,
    })
}
