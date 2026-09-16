//! Strict Taproot Bitcoin HTLC reconstruction, observation, signing and broadcast.
//! Backends are trusted chain readers; callers choose network and confirmation policy.
use crate::Client;
use crate::Error;
use crate::Result;
use crate::SwapMetadata;
use crate::types::GetSwapResponse;
use bitcoin::Address;
use bitcoin::Amount;
use bitcoin::BlockHash;
use bitcoin::FeeRate;
use bitcoin::Network;
use bitcoin::OutPoint;
use bitcoin::ScriptBuf;
use bitcoin::Sequence;
use bitcoin::TapLeafHash;
use bitcoin::TapSighashType;
use bitcoin::Transaction;
use bitcoin::TxIn;
use bitcoin::TxOut;
use bitcoin::Txid;
use bitcoin::Witness;
use bitcoin::absolute::LockTime;
use bitcoin::hashes::Hash;
use bitcoin::hashes::hash160;
use bitcoin::opcodes::all::*;
use bitcoin::script::Builder;
use bitcoin::secp256k1::Keypair;
use bitcoin::secp256k1::Message;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::secp256k1::SecretKey;
use bitcoin::secp256k1::XOnlyPublicKey;
use bitcoin::sighash::Prevouts;
use bitcoin::sighash::SighashCache;
use bitcoin::taproot::LeafVersion;
use bitcoin::taproot::TaprootBuilder;
use bitcoin::taproot::TaprootSpendInfo;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;

mod esplora;
pub use esplora::EsploraBackend;
#[cfg(feature = "electrum")]
mod electrum;
#[cfg(feature = "electrum")]
pub use electrum::ElectrumBackend;

pub type ChainFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Confirmed height is None for mempool outputs. Spent outputs are never included.
#[derive(Clone, Debug)]
pub struct BitcoinUtxo {
    pub outpoint: OutPoint,
    pub output: TxOut,
    pub confirmed_height: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct BitcoinObservation {
    pub tip: BlockHash,
    pub height: u32,
    pub median_time_past: u32,
    pub utxos: Vec<BitcoinUtxo>,
}

/// Implementations must verify their network, exclude spent outputs, return
/// actual transaction outputs, and reject an observation spanning a tip change.
pub trait BitcoinBackend: Send + Sync {
    fn network(&self) -> Network;
    fn observe<'a>(&'a self, address: &'a Address) -> ChainFuture<'a, BitcoinObservation>;
    fn transaction(&self, id: Txid) -> ChainFuture<'_, Option<Transaction>>;
    fn fee_rate(&self, confirmation_target: u16) -> ChainFuture<'_, FeeRate>;
    fn broadcast<'a>(&'a self, tx: &'a Transaction) -> ChainFuture<'a, Txid>;
}

#[derive(Clone, Debug)]
pub struct BitcoinHtlc {
    pub network: Network,
    pub hash_lock: [u8; 20],
    pub claimer: XOnlyPublicKey,
    pub refunder: XOnlyPublicKey,
    pub refund_locktime: u32,
    /// Version 1 enforces exactly 32 preimage bytes. Older versions are rejected.
    pub script_version: i64,
}

#[derive(Clone, Debug)]
pub struct BitcoinHtlcScripts {
    pub claim: ScriptBuf,
    pub refund: ScriptBuf,
    pub spend_info: TaprootSpendInfo,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidSwap(message.into())
}
fn decode(error: impl std::fmt::Display) -> Error {
    Error::Decode(error.to_string())
}
fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}

impl BitcoinHtlc {
    pub fn scripts(&self) -> Result<BitcoinHtlcScripts> {
        if self.script_version != 1 || self.refund_locktime == 0 {
            return Err(invalid("unsupported Bitcoin script version or locktime"));
        }
        let claim = Builder::new()
            .push_x_only_key(&self.claimer)
            .push_opcode(OP_CHECKSIGVERIFY)
            .push_opcode(OP_SIZE)
            .push_int(32)
            .push_opcode(OP_EQUALVERIFY)
            .push_opcode(OP_HASH160)
            .push_slice(self.hash_lock)
            .push_opcode(OP_EQUAL)
            .into_script();
        let refund = Builder::new()
            .push_int(self.refund_locktime.into())
            .push_opcode(OP_CLTV)
            .push_opcode(OP_DROP)
            .push_x_only_key(&self.refunder)
            .push_opcode(OP_CHECKSIG)
            .into_script();
        let internal = XOnlyPublicKey::from_str(
            "50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0",
        )
        .map_err(decode)?;
        let spend_info = TaprootBuilder::new()
            .add_leaf(1, claim.clone())
            .map_err(decode)?
            .add_leaf(1, refund.clone())
            .map_err(decode)?
            .finalize(&Secp256k1::new(), internal)
            .map_err(|_| invalid("invalid Bitcoin Taproot tree"))?;
        Ok(BitcoinHtlcScripts {
            claim,
            refund,
            spend_info,
        })
    }
    pub fn address(&self) -> Result<Address> {
        Ok(Address::p2tr_tweaked(
            self.scripts()?.spend_info.output_key(),
            self.network,
        ))
    }
    pub fn expired(&self, observation: &BitcoinObservation) -> bool {
        if self.refund_locktime < 500_000_000 {
            observation.height >= self.refund_locktime
        } else {
            observation.median_time_past > self.refund_locktime
        }
    }
}

/// Preimages and signing keys are intentionally excluded from diagnostics.
pub enum BitcoinSpend {
    Claim { preimage: [u8; 32] },
    Refund,
}
impl std::fmt::Debug for BitcoinSpend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Claim { .. } => "Claim { [REDACTED] }",
            Self::Refund => "Refund",
        })
    }
}

#[derive(Clone, Debug)]
pub struct BitcoinSpendOptions {
    pub destination: String,
    /// None queries the backend using `confirmation_target`.
    pub fee_rate: Option<FeeRate>,
    pub maximum_fee: Amount,
    pub minimum_confirmations: u32,
    pub confirmation_target: u16,
    pub timeout: Duration,
}

/// Signed transaction contains a public claim preimage; Debug omits the transaction.
#[derive(Clone)]
pub struct BitcoinSpendReceipt {
    pub transaction: Transaction,
    pub fee: Amount,
}
impl std::fmt::Debug for BitcoinSpendReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitcoinSpendReceipt")
            .field("txid", &self.transaction.compute_txid())
            .field("fee", &self.fee)
            .finish()
    }
}

/// Build a spend of independently observed, unspent HTLC outputs. Verifies key,
/// preimage, maturity, confirmations, exact script, duplicates, fees and dust.
#[allow(clippy::too_many_arguments)]
pub fn build_bitcoin_spend(
    htlc: &BitcoinHtlc,
    observation: &BitcoinObservation,
    key: &SecretKey,
    spend: &BitcoinSpend,
    destination: &str,
    fee_rate: FeeRate,
    minimum_confirmations: u32,
    minimum_amount: Amount,
    maximum_fee: Amount,
) -> Result<BitcoinSpendReceipt> {
    let destination = Address::from_str(destination)
        .map_err(decode)?
        .require_network(htlc.network)
        .map_err(decode)?;
    let scripts = htlc.scripts()?;
    let prev_script = htlc.address()?.script_pubkey();
    let secp = Secp256k1::new();
    let pair = Keypair::from_secret_key(&secp, key);
    let (public, _) = pair.x_only_public_key();
    let (script, locktime, sequence) = match spend {
        BitcoinSpend::Claim { preimage } => {
            if hash160::Hash::hash(preimage).to_byte_array() != htlc.hash_lock
                || public != htlc.claimer
            {
                return Err(invalid("Bitcoin claim key or preimage mismatch"));
            }
            if htlc.expired(observation) {
                return Err(invalid("Bitcoin claim window has expired"));
            }
            (&scripts.claim, LockTime::ZERO, Sequence::MAX)
        }
        BitcoinSpend::Refund => {
            if public != htlc.refunder || !htlc.expired(observation) {
                return Err(invalid(
                    "Bitcoin refund key mismatch or timelock not mature",
                ));
            }
            (
                &scripts.refund,
                LockTime::from_consensus(htlc.refund_locktime),
                Sequence::ENABLE_RBF_NO_LOCKTIME,
            )
        }
    };
    let mut utxos: Vec<_> = observation
        .utxos
        .iter()
        .filter(|utxo| {
            let confirmations = utxo
                .confirmed_height
                .and_then(|h| observation.height.checked_sub(h))
                .map_or(0, |n| n.saturating_add(1));
            confirmations >= minimum_confirmations
        })
        .collect();
    utxos.sort_by_key(|u| u.outpoint);
    if utxos.is_empty() || utxos.windows(2).any(|w| w[0].outpoint == w[1].outpoint) {
        return Err(invalid(
            "missing confirmed Bitcoin outputs or duplicate outpoint",
        ));
    }
    let mut amount = 0u64;
    for utxo in &utxos {
        if utxo.output.script_pubkey != prev_script {
            return Err(invalid("funding output does not match Bitcoin HTLC"));
        }
        amount = amount
            .checked_add(utxo.output.value.to_sat())
            .filter(|v| *v <= Amount::MAX_MONEY.to_sat())
            .ok_or_else(|| invalid("invalid Bitcoin funding amount"))?;
    }
    if amount < minimum_amount.to_sat() {
        return Err(invalid("Bitcoin HTLC is underfunded"));
    }
    let control = scripts
        .spend_info
        .control_block(&(script.clone(), LeafVersion::TapScript))
        .ok_or_else(|| invalid("missing Taproot control block"))?
        .serialize();
    let witness_for = |signature: &[u8]| {
        let mut witness = Witness::new();
        if let BitcoinSpend::Claim { preimage } = spend {
            witness.push(preimage);
        }
        witness.push(signature);
        witness.push(script.as_bytes());
        witness.push(&control);
        witness
    };
    let mut tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: locktime,
        input: utxos
            .iter()
            .map(|u| TxIn {
                previous_output: u.outpoint,
                script_sig: ScriptBuf::new(),
                sequence,
                witness: witness_for(&[0; 64]),
            })
            .collect(),
        output: vec![TxOut {
            value: Amount::from_sat(amount),
            script_pubkey: destination.script_pubkey(),
        }],
    };
    let fee = fee_rate
        .checked_mul_by_weight(tx.weight())
        .ok_or_else(|| invalid("Bitcoin fee overflow"))?;
    if fee == Amount::ZERO || fee > maximum_fee || fee.to_sat() >= amount {
        return Err(invalid(
            "Bitcoin fee exceeds the spending limit or available funds",
        ));
    }
    tx.output[0].value = Amount::from_sat(amount - fee.to_sat());
    if tx.output[0].value < tx.output[0].script_pubkey.minimal_non_dust() {
        return Err(invalid("Bitcoin output would be dust"));
    }
    let prevouts: Vec<_> = utxos.iter().map(|u| u.output.clone()).collect();
    let leaf = TapLeafHash::from_script(script, LeafVersion::TapScript);
    for i in 0..utxos.len() {
        let sighash = SighashCache::new(&tx)
            .taproot_script_spend_signature_hash(
                i,
                &Prevouts::All(&prevouts),
                leaf,
                TapSighashType::Default,
            )
            .map_err(decode)?;
        let signature =
            secp.sign_schnorr_no_aux_rand(&Message::from_digest(sighash.to_byte_array()), &pair);
        tx.input[i].witness = witness_for(signature.as_ref());
    }
    Ok(BitcoinSpendReceipt {
        transaction: tx,
        fee,
    })
}

fn parse_key(value: &str) -> Result<XOnlyPublicKey> {
    let bytes = hex::decode(value.trim_start_matches("0x")).map_err(decode)?;
    if bytes.len() == 33 {
        bitcoin::secp256k1::PublicKey::from_slice(&bytes)
            .map(|p| p.x_only_public_key().0)
            .map_err(decode)
    } else {
        XOnlyPublicKey::from_slice(&bytes).map_err(decode)
    }
}
fn network(value: &str) -> Result<Network> {
    match value {
        "mainnet" | "bitcoin" => Ok(Network::Bitcoin),
        "testnet" => Ok(Network::Testnet),
        "signet" => Ok(Network::Signet),
        "regtest" => Ok(Network::Regtest),
        _ => Err(invalid("unknown Bitcoin network")),
    }
}

impl Client {
    /// Reconstruct a stored swap's Bitcoin side using locally derived ownership.
    pub fn bitcoin_htlc(&self, id: &str) -> Result<BitcoinHtlc> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        let params = self
            .signer
            .as_ref()
            .ok_or_else(|| invalid("signer missing"))?
            .derive_swap_params(stored.key_index)?;
        let own = parse_key(&hex::encode(params.public_key))?;
        let (hash, address, version, timelock, network_name, claimer, refunder) =
            match &stored.response {
                GetSwapResponse::BitcoinToEvm(r) => (
                    &r.btc_hash_lock,
                    &r.btc_htlc_address,
                    r.btc_htlc_script_version,
                    r.btc_refund_locktime,
                    &r.network,
                    parse_key(&r.btc_server_pk)?,
                    own,
                ),
                GetSwapResponse::BtcToArkade(r) => (
                    &r.hash_lock,
                    &r.btc_htlc_address,
                    r.btc_htlc_script_version,
                    r.btc_refund_locktime,
                    &r.network,
                    parse_key(&r.server_vhtlc_pk)?,
                    own,
                ),
                GetSwapResponse::EvmToBitcoin(r) => {
                    if parse_key(&r.btc_user_claim_pk)? != own {
                        return Err(invalid("Bitcoin claim public key mismatch"));
                    }
                    (
                        &r.btc_hash_lock,
                        &r.btc_htlc_address,
                        r.btc_htlc_script_version,
                        r.btc_refund_locktime,
                        &r.network,
                        own,
                        parse_key(&r.btc_server_refund_pk)?,
                    )
                }
                _ => return Err(invalid("swap has no on-chain Bitcoin HTLC")),
            };
        let hash_lock: [u8; 20] = hex::decode(hash.trim_start_matches("0x"))
            .map_err(decode)?
            .try_into()
            .map_err(|_| invalid("Bitcoin hash lock must be HASH160"))?;
        if hash160::Hash::hash(&params.preimage).to_byte_array() != hash_lock {
            return Err(invalid("Bitcoin hash lock mismatch"));
        }
        let htlc = BitcoinHtlc {
            network: network(network_name)?,
            hash_lock,
            claimer,
            refunder,
            refund_locktime: u32::try_from(timelock).map_err(decode)?,
            script_version: version,
        };
        if htlc.address()?.to_string() != *address {
            return Err(invalid("Bitcoin HTLC address mismatch"));
        }
        Ok(htlc)
    }

    pub async fn claim_bitcoin(
        &self,
        id: &str,
        backend: &dyn BitcoinBackend,
        options: BitcoinSpendOptions,
    ) -> Result<BitcoinSpendReceipt> {
        self.spend_bitcoin(id, backend, options, true).await
    }
    pub async fn refund_bitcoin(
        &self,
        id: &str,
        backend: &dyn BitcoinBackend,
        options: BitcoinSpendOptions,
    ) -> Result<BitcoinSpendReceipt> {
        self.spend_bitcoin(id, backend, options, false).await
    }
    async fn spend_bitcoin(
        &self,
        id: &str,
        backend: &dyn BitcoinBackend,
        options: BitcoinSpendOptions,
        claim: bool,
    ) -> Result<BitcoinSpendReceipt> {
        tokio::time::timeout(options.timeout, async {
            let _execution = self.bitcoin_execution.lock().await;
            let stored = self
                .get_stored_swap(id)?
                .ok_or_else(|| invalid("swap record missing"))?;
            if claim != matches!(stored.response, GetSwapResponse::EvmToBitcoin(_)) {
                return Err(invalid("wrong Bitcoin spend direction"));
            }
            let htlc = self.bitcoin_htlc(id)?;
            if htlc.network != backend.network() {
                return Err(invalid("Bitcoin backend network mismatch"));
            }
            if let Some(hash) = &stored.metadata.btc_spend_txid
                && let Some(tx) = backend
                    .transaction(Txid::from_str(hash).map_err(decode)?)
                    .await?
            {
                let destination = Address::from_str(&options.destination)
                    .map_err(decode)?
                    .require_network(htlc.network)
                    .map_err(decode)?;
                if tx.compute_txid().to_string() != *hash
                    || tx.output.len() != 1
                    || tx.output[0].script_pubkey != destination.script_pubkey()
                {
                    return Err(invalid("persisted Bitcoin spend mismatch"));
                }
                return Ok(BitcoinSpendReceipt {
                    transaction: tx,
                    fee: Amount::from_sat(
                        stored
                            .metadata
                            .btc_spend_fee
                            .ok_or_else(|| invalid("persisted Bitcoin fee missing"))?,
                    ),
                });
            }
            let params = self
                .signer
                .as_ref()
                .ok_or_else(|| invalid("signer missing"))?
                .derive_swap_params(stored.key_index)?;
            let observation = backend.observe(&htlc.address()?).await?;
            let rate = match options.fee_rate {
                Some(rate) => rate,
                None => backend.fee_rate(options.confirmation_target).await?,
            };
            let minimum = if let GetSwapResponse::EvmToBitcoin(r) = &stored.response {
                Amount::from_sat(r.target_amount.parse().map_err(decode)?)
            } else {
                Amount::ZERO
            };
            let spend = if claim {
                BitcoinSpend::Claim {
                    preimage: params.preimage,
                }
            } else {
                BitcoinSpend::Refund
            };
            let receipt = build_bitcoin_spend(
                &htlc,
                &observation,
                &SecretKey::from_slice(&params.secret).map_err(decode)?,
                &spend,
                &options.destination,
                rate,
                options.minimum_confirmations,
                minimum,
                options.maximum_fee,
            )?;
            let hash = receipt.transaction.compute_txid();
            if stored
                .metadata
                .btc_spend_txid
                .is_some_and(|old| old != hash.to_string())
            {
                return Err(invalid(
                    "previous Bitcoin submission is unresolved; refusing a different spend",
                ));
            }
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    btc_spend_txid: Some(hash.to_string()),
                    btc_spend_fee: Some(receipt.fee.to_sat()),
                    ..Default::default()
                },
            )?;
            if backend.broadcast(&receipt.transaction).await? != hash {
                return Err(invalid(
                    "Bitcoin broadcast returned a different transaction ID",
                ));
            }
            Ok(receipt)
        })
        .await
        .map_err(|_| {
            Error::Timeout("Bitcoin spend timed out; inspect the stored transaction".into())
        })?
    }
}

#[cfg(test)]
mod tests;
