use super::*;

fn fixture() -> (
    BitcoinHtlc,
    BitcoinObservation,
    SecretKey,
    SecretKey,
    String,
) {
    let secp = Secp256k1::new();
    let claim = SecretKey::from_slice(&[1; 32]).unwrap();
    let refund = SecretKey::from_slice(&[2; 32]).unwrap();
    let htlc = BitcoinHtlc {
        network: Network::Regtest,
        hash_lock: hash160::Hash::hash(&[42; 32]).to_byte_array(),
        claimer: Keypair::from_secret_key(&secp, &claim)
            .x_only_public_key()
            .0,
        refunder: Keypair::from_secret_key(&secp, &refund)
            .x_only_public_key()
            .0,
        refund_locktime: 1_700_000_000,
        script_version: 1,
    };
    let observation = BitcoinObservation {
        tip: BlockHash::all_zeros(),
        height: 100,
        median_time_past: htlc.refund_locktime - 1,
        utxos: vec![BitcoinUtxo {
            outpoint: OutPoint {
                txid: Txid::all_zeros(),
                vout: 0,
            },
            output: TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: htlc.address().unwrap().script_pubkey(),
            },
            confirmed_height: Some(99),
        }],
    };
    let destination = Address::p2tr(&secp, htlc.refunder, None, Network::Regtest).to_string();
    (htlc, observation, claim, refund, destination)
}
fn build(
    htlc: &BitcoinHtlc,
    obs: &BitcoinObservation,
    key: &SecretKey,
    spend: &BitcoinSpend,
    dest: &str,
) -> Result<BitcoinSpendReceipt> {
    build_bitcoin_spend(
        htlc,
        obs,
        key,
        spend,
        dest,
        FeeRate::from_sat_per_vb(2).unwrap(),
        2,
        0,
        Amount::from_sat(100_000),
        Amount::from_sat(10_000),
    )
}

#[test]
fn strict_script_addresses_match_typescript_vectors() {
    let (htlc, _, _, _, _) = fixture();
    assert_eq!(
        htlc.address().unwrap().to_string(),
        "bcrt1pals5upc3vwh8f55tvtrvvhx4pqfxzqx83esy0caftd3rh7ap8ruqv4l9af"
    );
    let htlc = BitcoinHtlc {
        network: Network::Signet,
        hash_lock: hex::decode("9befb12985069ca625bce37f13af8acbb66e46bb")
            .unwrap()
            .try_into()
            .unwrap(),
        claimer: parse_key("6c932b95705b07c4236a1abfabe283399774449914f6c4d7faeb30fd7f3c6b0e")
            .unwrap(),
        refunder: parse_key("d149150a0c344bae35cfe0cd237e50bd41ec56fe7c2a2f5f8509911ec8c5a0e2")
            .unwrap(),
        refund_locktime: 1769752815,
        script_version: 1,
    };
    assert_eq!(
        htlc.address().unwrap().to_string(),
        "tb1pmflkz8yplmk3v8htfmda2dqcnxgkj6fldrzslq90qc3y3h235spq6juwps"
    );
}

#[test]
fn claims_and_refunds_have_valid_signatures_control_blocks_and_exact_fees() {
    let (htlc, mut obs, claim, refund, dest) = fixture();
    for (key, spend, script) in [
        (
            claim,
            BitcoinSpend::Claim { preimage: [42; 32] },
            htlc.scripts().unwrap().claim,
        ),
        (refund, BitcoinSpend::Refund, htlc.scripts().unwrap().refund),
    ] {
        if matches!(spend, BitcoinSpend::Refund) {
            obs.median_time_past = htlc.refund_locktime + 1;
        }
        let receipt = build(&htlc, &obs, &key, &spend, &dest).unwrap();
        let tx = &receipt.transaction;
        assert_eq!(
            receipt.fee,
            FeeRate::from_sat_per_vb(2)
                .unwrap()
                .checked_mul_by_weight(tx.weight())
                .unwrap()
        );
        assert_eq!(tx.output[0].value + receipt.fee, Amount::from_sat(100_000));
        let stack: Vec<_> = tx.input[0].witness.iter().collect();
        let signature =
            bitcoin::secp256k1::schnorr::Signature::from_slice(stack[stack.len() - 3]).unwrap();
        let sighash = SighashCache::new(tx)
            .taproot_script_spend_signature_hash(
                0,
                &Prevouts::All(&[obs.utxos[0].output.clone()]),
                TapLeafHash::from_script(&script, LeafVersion::TapScript),
                TapSighashType::Default,
            )
            .unwrap();
        let secp = Secp256k1::new();
        secp.verify_schnorr(
            &signature,
            &Message::from_digest(sighash.to_byte_array()),
            &Keypair::from_secret_key(&secp, &key).x_only_public_key().0,
        )
        .unwrap();
        let control = bitcoin::taproot::ControlBlock::decode(stack.last().unwrap()).unwrap();
        assert!(
            control.verify_taproot_commitment(
                &secp,
                htlc.scripts()
                    .unwrap()
                    .spend_info
                    .output_key()
                    .to_x_only_public_key(),
                &script
            )
        );
        assert!(!format!("{receipt:?}").contains(&hex::encode([42; 32])));
    }
}

#[test]
fn signing_rejects_wrong_key_preimage_network_scripts_maturity_and_funding() {
    let (mut htlc, mut obs, claim, refund, dest) = fixture();
    let spend = BitcoinSpend::Claim { preimage: [42; 32] };
    assert!(build(&htlc, &obs, &refund, &spend, &dest).is_err());
    assert!(
        build(
            &htlc,
            &obs,
            &claim,
            &BitcoinSpend::Claim { preimage: [41; 32] },
            &dest
        )
        .is_err()
    );
    assert!(build(&htlc, &obs, &refund, &BitcoinSpend::Refund, &dest).is_err());
    obs.median_time_past = htlc.refund_locktime;
    assert!(build(&htlc, &obs, &refund, &BitcoinSpend::Refund, &dest).is_err());
    obs.median_time_past -= 1;
    htlc.script_version = 0;
    assert!(build(&htlc, &obs, &claim, &spend, &dest).is_err());
    htlc.script_version = 1;
    let wrong_dest =
        Address::p2tr(&Secp256k1::new(), htlc.refunder, None, Network::Bitcoin).to_string();
    assert!(build(&htlc, &obs, &claim, &spend, &wrong_dest).is_err());
    obs.utxos[0].confirmed_height = Some(100);
    assert!(build(&htlc, &obs, &claim, &spend, &dest).is_err());
    obs.utxos[0].confirmed_height = Some(99);
    obs.utxos[0].output.value = Amount::from_sat(99_999);
    assert!(build(&htlc, &obs, &claim, &spend, &dest).is_err());
    obs.utxos[0].output.value = Amount::from_sat(100_000);
    obs.utxos.push(obs.utxos[0].clone());
    assert!(build(&htlc, &obs, &claim, &spend, &dest).is_err());
    obs.utxos.pop();
    obs.utxos[0].output.script_pubkey = ScriptBuf::new();
    assert!(build(&htlc, &obs, &claim, &spend, &dest).is_err());
}

#[test]
fn dust_and_excessive_fees_are_rejected() {
    let (htlc, mut obs, claim, _, dest) = fixture();
    let spend = BitcoinSpend::Claim { preimage: [42; 32] };
    let result = build_bitcoin_spend(
        &htlc,
        &obs,
        &claim,
        &spend,
        &dest,
        FeeRate::from_sat_per_vb(1000).unwrap(),
        1,
        0,
        Amount::ZERO,
        Amount::from_sat(100),
    );
    assert!(result.is_err());
    obs.utxos[0].output.value = Amount::from_sat(400);
    assert!(
        build_bitcoin_spend(
            &htlc,
            &obs,
            &claim,
            &spend,
            &dest,
            FeeRate::from_sat_per_vb(1).unwrap(),
            1,
            0,
            Amount::ZERO,
            Amount::from_sat(1000)
        )
        .is_err()
    );
}

/// Reports outputs spent in the mempool as spent, as Esplora does, and evicts a
/// mempool spend when a later broadcast conflicts with it.
struct Backend {
    observation: BitcoinObservation,
    with_history: bool,
    fundings: std::sync::Mutex<Vec<BitcoinHistoryTransaction>>,
    mempool: std::sync::Mutex<Vec<BitcoinHistoryTransaction>>,
    broadcasts: std::sync::Mutex<Vec<Transaction>>,
    interrupt: std::sync::atomic::AtomicBool,
    reject: std::sync::atomic::AtomicBool,
}
impl Backend {
    fn new(observation: BitcoinObservation, with_history: bool) -> Self {
        let backend = Self {
            observation,
            with_history,
            fundings: std::sync::Mutex::new(vec![]),
            mempool: std::sync::Mutex::new(vec![]),
            broadcasts: std::sync::Mutex::new(vec![]),
            interrupt: std::sync::atomic::AtomicBool::new(false),
            reject: std::sync::atomic::AtomicBool::new(false),
        };
        backend.fund(100_000, Some(99));
        backend
    }
    fn fund(&self, sats: u64, confirmed_height: Option<u32>) {
        let mut fundings = self.fundings.lock().unwrap();
        let transaction = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: LockTime::from_consensus(fundings.len() as u32),
            input: vec![TxIn::default()],
            output: vec![TxOut {
                value: Amount::from_sat(sats),
                script_pubkey: self.observation.utxos[0].output.script_pubkey.clone(),
            }],
        };
        fundings.push(BitcoinHistoryTransaction {
            transaction,
            confirmed_height,
        });
    }
    fn confirm(&self, txid: Txid, height: u32) {
        for entry in self.mempool.lock().unwrap().iter_mut() {
            if entry.transaction.compute_txid() == txid {
                entry.confirmed_height = Some(height);
            }
        }
    }
    fn submissions(&self) -> usize {
        self.broadcasts.lock().unwrap().len()
    }
    fn current(&self) -> BitcoinHistory {
        let fundings = self.fundings.lock().unwrap().clone();
        let mempool = self.mempool.lock().unwrap().clone();
        let spent: Vec<OutPoint> = mempool
            .iter()
            .flat_map(|e| e.transaction.input.iter().map(|i| i.previous_output))
            .collect();
        let utxos = fundings
            .iter()
            .map(|e| BitcoinUtxo {
                outpoint: OutPoint {
                    txid: e.transaction.compute_txid(),
                    vout: 0,
                },
                output: e.transaction.output[0].clone(),
                confirmed_height: e.confirmed_height,
            })
            .filter(|u| !spent.contains(&u.outpoint))
            .collect();
        BitcoinHistory {
            observation: BitcoinObservation {
                utxos,
                ..self.observation.clone()
            },
            transactions: fundings.into_iter().chain(mempool).collect(),
        }
    }
}
impl BitcoinBackend for Backend {
    fn network(&self) -> Network {
        Network::Regtest
    }
    fn history<'a>(&'a self, _: &'a Address) -> ChainFuture<'a, BitcoinHistory> {
        Box::pin(async {
            if self.with_history {
                Ok(self.current())
            } else {
                Err(invalid(NO_HISTORY))
            }
        })
    }
    fn observe<'a>(&'a self, _: &'a Address) -> ChainFuture<'a, BitcoinObservation> {
        Box::pin(async { Ok(self.current().observation) })
    }
    fn transaction(&self, id: Txid) -> ChainFuture<'_, Option<Transaction>> {
        Box::pin(async move {
            Ok(self
                .broadcasts
                .lock()
                .unwrap()
                .iter()
                .find(|tx| tx.compute_txid() == id)
                .cloned())
        })
    }
    fn fee_rate(&self, _: u16) -> ChainFuture<'_, FeeRate> {
        Box::pin(async { Ok(FeeRate::from_sat_per_vb(2).unwrap()) })
    }
    fn broadcast<'a>(&'a self, tx: &'a Transaction) -> ChainFuture<'a, Txid> {
        Box::pin(async move {
            if self.reject.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(invalid("rejected by the node"));
            }
            self.broadcasts.lock().unwrap().push(tx.clone());
            let mut mempool = self.mempool.lock().unwrap();
            mempool.retain(|e| {
                !e.transaction.input.iter().any(|old| {
                    tx.input
                        .iter()
                        .any(|new| new.previous_output == old.previous_output)
                })
            });
            mempool.push(BitcoinHistoryTransaction {
                transaction: tx.clone(),
                confirmed_height: None,
            });
            if self
                .interrupt
                .swap(false, std::sync::atomic::Ordering::Relaxed)
            {
                return Err(transport("connection closed after submission"));
            }
            Ok(tx.compute_txid())
        })
    }
}

const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn claim_setup(
    with_history: bool,
) -> (
    Client,
    std::sync::Arc<crate::InMemorySwapStorage>,
    Backend,
    BitcoinSpendOptions,
) {
    use crate::InMemorySwapStorage;
    use crate::Signer;
    use crate::StoredSwap;
    use crate::SwapStorage;
    use serde_json::json;
    let params = Signer::from_mnemonic(MNEMONIC)
        .unwrap()
        .derive_swap_params(0)
        .unwrap();
    let (mut htlc, mut observation, _, _, destination) = fixture();
    htlc.hash_lock = hash160::Hash::hash(&params.preimage).to_byte_array();
    htlc.claimer = parse_key(&hex::encode(params.public_key)).unwrap();
    observation.utxos[0].output.script_pubkey = htlc.address().unwrap().script_pubkey();
    let mut fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let r = &mut fixture["evm_to_bitcoin"];
    r["id"] = json!("bitcoin");
    r["btc_hash_lock"] = json!(hex::encode(htlc.hash_lock));
    r["btc_htlc_address"] = json!(htlc.address().unwrap().to_string());
    r["btc_user_claim_pk"] = json!(htlc.claimer.to_string());
    r["btc_server_refund_pk"] = json!(htlc.refunder.to_string());
    r["network"] = json!("regtest");
    r["btc_refund_locktime"] = json!(htlc.refund_locktime);
    r["target_amount"] = json!("100000");
    let storage = std::sync::Arc::new(InMemorySwapStorage::new());
    storage
        .store_swap(
            StoredSwap::new(
                0,
                serde_json::from_value(r.clone()).unwrap(),
                SwapMetadata::default(),
            )
            .unwrap(),
        )
        .unwrap();
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(storage.clone())
        .build()
        .unwrap();
    let options = BitcoinSpendOptions {
        destination,
        fee_rate: None,
        maximum_fee: Amount::from_sat(5000),
        minimum_confirmations: 2,
        minimum_remaining_blocks: 0,
        confirmation_target: 6,
        timeout: Duration::from_secs(5),
    };
    (
        client,
        storage,
        Backend::new(observation, with_history),
        options,
    )
}

fn journaled(storage: &crate::InMemorySwapStorage) -> (Option<String>, Option<u64>) {
    use crate::SwapStorage;
    let metadata = storage.get_swap("bitcoin").unwrap().unwrap().metadata;
    (metadata.btc_spend_txid, metadata.btc_spend_fee)
}

fn point_journal_elsewhere(storage: &crate::InMemorySwapStorage) {
    use crate::SwapStorage;
    storage
        .update_metadata(
            "bitcoin",
            SwapMetadata {
                btc_spend_txid: Some(Txid::all_zeros().to_string()),
                btc_spend_fee: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
}

#[tokio::test]
async fn interrupted_client_claim_resumes_the_persisted_transaction_without_another_broadcast() {
    for with_history in [true, false] {
        let (client, storage, backend, options) = claim_setup(with_history);
        backend
            .interrupt
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            client
                .claim_bitcoin("bitcoin", &backend, options.clone())
                .await
                .is_err()
        );
        assert!(journaled(&storage).0.is_some());
        let restarted = Client::builder()
            .mnemonic(MNEMONIC)
            .storage(storage)
            .build()
            .unwrap();
        let receipt = restarted
            .claim_bitcoin("bitcoin", &backend, options)
            .await
            .unwrap();
        assert_eq!(
            receipt.transaction.compute_txid(),
            backend.broadcasts.lock().unwrap()[0].compute_txid()
        );
        assert_eq!(backend.submissions(), 1);
    }
}

#[tokio::test]
async fn pending_claim_is_replaced_at_a_higher_explicit_fee_rate() {
    let (client, storage, mut backend, mut options) = claim_setup(true);
    options.fee_rate = Some(FeeRate::from_sat_per_vb(2).unwrap());
    let first = client
        .claim_bitcoin("bitcoin", &backend, options.clone())
        .await
        .unwrap();
    options.fee_rate = Some(FeeRate::from_sat_per_vb(10).unwrap());
    // A fresh claim this close to the window is refused; a bump of a public preimage is not.
    options.minimum_remaining_blocks = 1000;
    let bumped = client
        .claim_bitcoin("bitcoin", &backend, options.clone())
        .await
        .unwrap();
    let (t1, t2) = (&first.transaction, &bumped.transaction);
    assert_ne!(t2.compute_txid(), t1.compute_txid());
    assert_eq!(
        t2.input
            .iter()
            .map(|i| i.previous_output)
            .collect::<Vec<_>>(),
        t1.input
            .iter()
            .map(|i| i.previous_output)
            .collect::<Vec<_>>()
    );
    assert_eq!(t2.output[0].script_pubkey, t1.output[0].script_pubkey);
    assert!(bumped.fee.to_sat() >= first.fee.to_sat() + t2.vsize() as u64);
    assert_eq!(backend.submissions(), 2);
    assert_eq!(
        journaled(&storage),
        (
            Some(t2.compute_txid().to_string()),
            Some(bumped.fee.to_sat())
        )
    );
    for rate in [10, 2] {
        options.fee_rate = Some(FeeRate::from_sat_per_vb(rate).unwrap());
        let again = client
            .claim_bitcoin("bitcoin", &backend, options.clone())
            .await
            .unwrap();
        assert_eq!(again.transaction.compute_txid(), t2.compute_txid());
        assert_eq!(again.fee, bumped.fee);
    }
    options.fee_rate = None;
    let again = client
        .claim_bitcoin("bitcoin", &backend, options.clone())
        .await
        .unwrap();
    assert_eq!(again.transaction.compute_txid(), t2.compute_txid());
    assert_eq!(backend.submissions(), 2);
    // The fixture's median time sits one second before the locktime.
    backend.observation.median_time_past += 2;
    options.fee_rate = Some(FeeRate::from_sat_per_vb(20).unwrap());
    let late = client
        .claim_bitcoin("bitcoin", &backend, options.clone())
        .await
        .unwrap();
    assert_ne!(late.transaction.compute_txid(), t2.compute_txid());
    assert_eq!(backend.submissions(), 3);
    // A bump over the fee cap cannot be built; the pending claim stands.
    options.fee_rate = Some(FeeRate::from_sat_per_vb(1000).unwrap());
    let capped = client
        .claim_bitcoin("bitcoin", &backend, options)
        .await
        .unwrap();
    assert_eq!(
        capped.transaction.compute_txid(),
        late.transaction.compute_txid()
    );
    assert_eq!(backend.submissions(), 3);
}

#[tokio::test]
async fn confirmed_own_spend_is_adopted_when_the_journal_points_elsewhere() {
    let (client, storage, backend, options) = claim_setup(true);
    let first = client
        .claim_bitcoin("bitcoin", &backend, options.clone())
        .await
        .unwrap();
    let txid = first.transaction.compute_txid();
    backend.confirm(txid, 100);
    // Dust that arrives later cannot be swept and must not hide the confirmed claim.
    backend.fund(300, Some(99));
    point_journal_elsewhere(&storage);
    let adopted = client
        .claim_bitcoin("bitcoin", &backend, options)
        .await
        .unwrap();
    assert_eq!(adopted.transaction.compute_txid(), txid);
    assert_eq!(adopted.fee, first.fee);
    assert_eq!(
        journaled(&storage),
        (Some(txid.to_string()), Some(first.fee.to_sat()))
    );
    assert_eq!(backend.submissions(), 1);
}

#[tokio::test]
async fn journaled_spend_sweeps_outputs_that_arrive_later() {
    let (client, storage, backend, options) = claim_setup(true);
    let first = client
        .claim_bitcoin("bitcoin", &backend, options.clone())
        .await
        .unwrap();
    let t1 = first.transaction.compute_txid();
    assert_eq!(first.transaction.input.len(), 1);
    backend.fund(20_000, Some(99));
    let pending = client
        .claim_bitcoin("bitcoin", &backend, options.clone())
        .await
        .unwrap();
    assert_eq!(pending.transaction.compute_txid(), t1);
    assert_eq!(backend.submissions(), 1);
    backend.confirm(t1, 100);
    backend
        .reject
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let rejected = client
        .claim_bitcoin("bitcoin", &backend, options.clone())
        .await
        .unwrap();
    assert_eq!(rejected.transaction.compute_txid(), t1);
    assert_eq!(journaled(&storage).0, Some(t1.to_string()));
    backend
        .reject
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let swept = client
        .claim_bitcoin("bitcoin", &backend, options.clone())
        .await
        .unwrap();
    let t2 = swept.transaction.compute_txid();
    assert_ne!(t2, t1);
    assert_eq!(swept.transaction.input.len(), 1);
    assert_eq!(
        swept.transaction.input[0].previous_output.txid,
        backend.fundings.lock().unwrap()[1]
            .transaction
            .compute_txid()
    );
    assert_eq!(backend.submissions(), 2);
    assert_eq!(journaled(&storage).0, Some(t2.to_string()));
    let again = client
        .claim_bitcoin("bitcoin", &backend, options)
        .await
        .unwrap();
    assert_eq!(again.transaction.compute_txid(), t2);
    assert_eq!(backend.submissions(), 2);
}

#[tokio::test]
async fn backend_without_history_refuses_a_different_unresolved_spend() {
    let (client, storage, backend, options) = claim_setup(false);
    point_journal_elsewhere(&storage);
    let error = client
        .claim_bitcoin("bitcoin", &backend, options)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::InvalidSwap(m) if m.contains("unresolved")),
        "{error}"
    );
    assert_eq!(backend.submissions(), 0);
    assert_eq!(journaled(&storage).0, Some(Txid::all_zeros().to_string()));
}

#[tokio::test]
async fn esplora_observes_paginated_history_filters_spent_outputs_and_rejects_tip_changes() {
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (htlc, _, _, _, _) = fixture();
    let address = htlc.address().unwrap();
    let genesis = bitcoin::blockdata::constants::genesis_block(Network::Regtest);
    let hash = genesis.block_hash();
    let transactions: Vec<_> = (0..26)
        .map(|n| Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: LockTime::from_consensus(n),
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(1000),
                script_pubkey: address.script_pubkey(),
            }],
        })
        .collect();
    let history:Vec<_>=transactions.iter().map(|tx| json!({"txid":tx.compute_txid(),"version":2,"locktime":tx.lock_time.to_consensus_u32(),"vin":[],"vout":[],"size":tx.total_size(),"weight":tx.weight().to_wu(),"fee":0,"status":{"confirmed":true,"block_height":0,"block_hash":hash,"block_time":genesis.header.time}})).collect();
    let reorg = Arc::new(AtomicBool::new(false));
    let tips = Arc::new(AtomicUsize::new(0));
    let tip_count = tips.clone();
    let changing = reorg.clone();
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("GET"))
        .respond_with(move |request: &wiremock::Request| {
            let path = request.url.path();
            if path == "/block-height/0" {
                return ResponseTemplate::new(200).set_body_string(hash.to_string());
            }
            if path == "/blocks/tip/height" {
                return ResponseTemplate::new(200).set_body_string("0");
            }
            if path == "/blocks/tip/hash" {
                let n = tip_count.fetch_add(1, Ordering::Relaxed);
                return ResponseTemplate::new(200).set_body_string(
                    if changing.load(Ordering::Relaxed) && n % 2 == 1 {
                        BlockHash::all_zeros().to_string()
                    } else {
                        hash.to_string()
                    },
                );
            }
            if path == format!("/block/{hash}/header") {
                return ResponseTemplate::new(200)
                    .set_body_string(bitcoin::consensus::encode::serialize_hex(&genesis.header));
            }
            if path.starts_with("/scripthash/") {
                return ResponseTemplate::new(200).set_body_json(if path.contains("/chain/") {
                    &history[25..]
                } else {
                    &history[..25]
                });
            }
            for (n, tx) in transactions.iter().enumerate() {
                if path == format!("/tx/{}/raw", tx.compute_txid()) {
                    return ResponseTemplate::new(200)
                        .set_body_bytes(bitcoin::consensus::serialize(tx));
                }
                if path == format!("/tx/{}/outspend/0", tx.compute_txid()) {
                    return ResponseTemplate::new(200).set_body_json(json!({"spent":n==0}));
                }
            }
            ResponseTemplate::new(404)
        })
        .mount(&server)
        .await;
    let backend = EsploraBackend::new(&server.uri(), Network::Regtest).unwrap();
    let observed = backend.observe(&address).await.unwrap();
    assert_eq!(observed.utxos.len(), 25);
    assert_eq!(observed.median_time_past, genesis.header.time);
    tips.store(0, Ordering::Relaxed);
    reorg.store(true, Ordering::Relaxed);
    assert!(backend.observe(&address).await.is_err());
    let wrong_network = EsploraBackend::new(&server.uri(), Network::Bitcoin).unwrap();
    assert!(wrong_network.observe(&address).await.is_err());
}

#[cfg(feature = "electrum")]
#[tokio::test]
async fn electrum_reads_actual_funding_outputs_and_converts_btc_per_kb_fees() {
    use serde_json::Value;
    use serde_json::json;
    use std::io::BufRead;
    use std::io::BufReader;
    use std::io::Write;
    let (htlc, _, _, _, _) = fixture();
    let address = htlc.address().unwrap();
    let tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn::default()],
        output: vec![TxOut {
            value: Amount::from_sat(1000),
            script_pubkey: address.script_pubkey(),
        }],
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("tcp://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let header = bitcoin::consensus::encode::serialize_hex(
            &bitcoin::blockdata::constants::genesis_block(Network::Regtest).header,
        );
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let request: Value = serde_json::from_str(&line).unwrap();
            let result = match request["method"].as_str().unwrap() {
                "blockchain.block.header" => json!(header),
                "blockchain.headers.subscribe" => json!({"height":0,"hex":header}),
                "blockchain.scripthash.listunspent" => {
                    json!([{"tx_hash":tx.compute_txid(),"tx_pos":0,"value":1000,"height":0}])
                }
                "blockchain.transaction.get" => {
                    json!(bitcoin::consensus::encode::serialize_hex(&tx))
                }
                "blockchain.estimatefee" => json!(0.00002),
                method => panic!("unexpected method {method}"),
            };
            writeln!(
                stream,
                "{}",
                json!({"jsonrpc":"2.0","id":request["id"],"result":result})
            )
            .unwrap();
        }
    });
    let backend = ElectrumBackend::connect(url, Network::Regtest, 3)
        .await
        .unwrap();
    let observation = backend.observe(&address).await.unwrap();
    assert_eq!(observation.utxos.len(), 1);
    assert_eq!(observation.utxos[0].output.value, Amount::from_sat(1000));
    assert_eq!(
        backend.fee_rate(6).await.unwrap(),
        FeeRate::from_sat_per_vb(2).unwrap()
    );
    drop(backend);
    server.join().unwrap();
}
