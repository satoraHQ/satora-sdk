use super::*;
use crate::arkade::ArkadeBackend;
use crate::arkade::StrictVhtlcScript;
use crate::bitcoin::BitcoinBackend;
use bitcoin::Address;
use bitcoin::Amount;
use bitcoin::OutPoint;
use bitcoin::Psbt;
use bitcoin::Script;
use bitcoin::Witness;
use bitcoin::consensus::deserialize;
use std::collections::HashSet;
use std::sync::Arc;

pub struct ArkadeReader {
    backend: Arc<dyn ArkadeBackend>,
    clock: Arc<dyn BitcoinBackend>,
}
impl ArkadeReader {
    pub fn new(backend: Arc<dyn ArkadeBackend>, clock: Arc<dyn BitcoinBackend>) -> Self {
        Self { backend, clock }
    }
}
impl ContractReader for ArkadeReader {
    fn can_observe(&self, reference: &ContractRef) -> bool {
        matches!(reference,ContractRef::Arkade {network,..} if *network==self.clock.network())
    }
    fn observe<'a>(&'a self, reference: &'a ContractRef) -> ObservationFuture<'a, Observation> {
        Box::pin(async move {
            let ContractRef::Arkade {
                options,
                network,
                expected_sats,
            } = reference
            else {
                return Err(invalid("Arkade reader received another ledger"));
            };
            if !self.can_observe(reference) || *expected_sats == 0 {
                return Err(invalid("invalid Arkade observation network or amount"));
            }
            let script = StrictVhtlcScript::new(options.clone(), *network)?;
            let info = self.backend.info().await?;
            if info.network != *network
                || (info.signer_pk.x_only_public_key().0 != options.server
                    && !info
                        .deprecated_signers
                        .iter()
                        .any(|signer| signer.pk.x_only_public_key().0 == options.server))
            {
                return Err(invalid("Arkade server identity mismatch"));
            }
            let address = Address::from_script(&script.script_pubkey(), *network)
                .map_err(|e| Error::Decode(e.to_string()))?;
            let clock = self.clock.observe(&address).await?;
            let vtxos = self.backend.vtxos(script.address()).await?;
            let (mut total, mut remaining) = (0u64, 0u64);
            let (mut claims, mut refunds, mut unknown) = (0, 0, 0);
            let mut preimage = None;
            let mut seen = HashSet::new();
            for vtxo in &vtxos {
                if vtxo.script != script.script_pubkey()
                    || !vtxo.assets.is_empty()
                    || vtxo.amount > Amount::MAX_MONEY
                    || !seen.insert(vtxo.outpoint)
                {
                    return Err(invalid("Arkade observation contains invalid VTXOs"));
                }
                total = total
                    .checked_add(vtxo.amount.to_sat())
                    .ok_or_else(|| invalid("Arkade amount overflow"))?;
                if !vtxo.is_spent && !vtxo.is_unrolled {
                    remaining = remaining
                        .checked_add(vtxo.amount.to_sat())
                        .ok_or_else(|| invalid("Arkade amount overflow"))?;
                    continue;
                }
                let Some(txid) = vtxo.spent_by else {
                    unknown += 1;
                    continue;
                };
                let Some(spend) = self.backend.virtual_transaction(txid).await? else {
                    unknown += 1;
                    continue;
                };
                if spend.unsigned_tx.compute_txid() != txid {
                    return Err(invalid("Arkade spending transaction ID mismatch"));
                }
                let (state, secret) = classify_spend(&spend, vtxo.outpoint, options)?;
                if secret.is_some() {
                    preimage = secret;
                }
                match state {
                    HtlcState::SpentClaim => claims += 1,
                    HtlcState::SpentRefund => refunds += 1,
                    _ => unknown += 1,
                }
            }
            let state = if total == 0 {
                HtlcState::Absent
            } else if remaining > 0 {
                if total < *expected_sats {
                    HtlcState::Invalid
                } else {
                    HtlcState::Confirmed
                }
            } else if unknown > 0 || (claims > 0 && refunds > 0) {
                HtlcState::SpentUnknown
            } else if claims > 0 {
                HtlcState::SpentClaim
            } else {
                HtlcState::SpentRefund
            };
            Ok(Observation {
                state,
                point: Some(ChainPoint {
                    height: u64::from(clock.height),
                    hash: clock.tip.to_string(),
                    timestamp: u64::from(clock.median_time_past),
                }),
                funded_amount: total.to_string(),
                remaining_amount: remaining.to_string(),
                preimage,
            })
        })
    }
}
/// Only inspect the input spending this output. A preimage on another input
/// must not turn this particular HTLC's refund into a claim observation.
pub(super) fn classify_spend(
    psbt: &Psbt,
    outpoint: OutPoint,
    options: &crate::arkade::StrictVhtlcOptions,
) -> Result<(HtlcState, Option<VerifiedPreimage>)> {
    let Some(index) = psbt
        .unsigned_tx
        .input
        .iter()
        .position(|input| input.previous_output == outpoint)
    else {
        return Ok((HtlcState::SpentUnknown, None));
    };
    let input = psbt
        .inputs
        .get(index)
        .ok_or_else(|| invalid("Arkade PSBT input missing"))?;
    let hash = HashLock::Hash160(options.preimage_hash.to_byte_array());
    let key = bitcoin::psbt::raw::Key {
        type_value: 222,
        key: ark_rs::core::VTXO_CONDITION_KEY.to_vec(),
    };
    let condition = input
        .unknown
        .get(&key)
        .map(|bytes| deserialize::<Witness>(bytes))
        .transpose()
        .map_err(|e| Error::Decode(e.to_string()))?;
    let preimage = condition
        .as_ref()
        .and_then(|witness| witness.iter().find_map(|bytes| hash.preimage(bytes)))
        .or_else(|| {
            input
                .final_script_witness
                .as_ref()
                .and_then(|witness| witness.iter().find_map(|bytes| hash.preimage(bytes)))
        });
    let tree = StrictVhtlcScript::new(options.clone(), Network::Bitcoin)?;
    let output_key = tree
        .taproot_spend_info()
        .output_key()
        .to_x_only_public_key();
    let mut executed = Vec::new();
    if let Some(witness) = &input.final_script_witness {
        let mut elements: Vec<_> = witness.iter().collect();
        if elements
            .last()
            .is_some_and(|item| item.first() == Some(&0x50))
        {
            elements.pop();
        }
        if elements.len() >= 2 {
            let control = elements.pop().unwrap();
            let script = Script::from_bytes(elements.pop().unwrap());
            if bitcoin::taproot::ControlBlock::decode(control).is_ok_and(|control| {
                control.verify_taproot_commitment(
                    &bitcoin::secp256k1::Secp256k1::verification_only(),
                    output_key,
                    script,
                )
            }) {
                executed.push(script.to_owned());
            }
        }
    } else {
        for (control, (script, version)) in &input.tap_scripts {
            let leaf = bitcoin::TapLeafHash::from_script(script, *version);
            if control.verify_taproot_commitment(
                &bitcoin::secp256k1::Secp256k1::verification_only(),
                output_key,
                script,
            ) && input.tap_script_sigs.keys().any(|(key, hash)| {
                *hash == leaf && [options.sender, options.receiver, options.server].contains(key)
            }) {
                executed.push(script.clone());
            }
        }
    }
    if executed.len() != 1 {
        return Ok((HtlcState::SpentUnknown, preimage));
    }
    let script = &executed[0];
    if (script == &options.claim_script() || script == &options.unilateral_claim_script())
        && preimage.is_some()
    {
        return Ok((HtlcState::SpentClaim, preimage));
    }
    let refunds = [
        options.refund_script(),
        options.refund_without_receiver_script(),
        options.unilateral_refund_script(),
        options.unilateral_refund_without_receiver_script(),
    ];
    if refunds.contains(script) {
        return Ok((HtlcState::SpentRefund, preimage));
    }
    Ok((HtlcState::SpentUnknown, preimage))
}
