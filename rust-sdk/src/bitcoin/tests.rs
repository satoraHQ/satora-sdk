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
    spend_setup(with_history, false, 100_000)
}

/// A claim targets `target` sats; a refund is a Bitcoin-to-EVM swap past its locktime.
fn spend_setup(
    with_history: bool,
    refund: bool,
    target: u64,
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
    let own = parse_key(&hex::encode(params.public_key)).unwrap();
    if refund {
        htlc.refunder = own;
        observation.median_time_past = htlc.refund_locktime + 1;
    } else {
        htlc.claimer = own;
    }
    observation.utxos[0].output.script_pubkey = htlc.address().unwrap().script_pubkey();
    let mut fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let r = &mut fixture[if refund {
        "bitcoin_to_evm"
    } else {
        "evm_to_bitcoin"
    }];
    r["id"] = json!("bitcoin");
    r["btc_hash_lock"] = json!(hex::encode(htlc.hash_lock));
    r["btc_htlc_address"] = json!(htlc.address().unwrap().to_string());
    r["btc_user_claim_pk"] = json!(htlc.claimer.to_string());
    r["btc_server_refund_pk"] = json!(htlc.refunder.to_string());
    r["btc_server_pk"] = json!(htlc.claimer.to_string());
    r["network"] = json!("regtest");
    r["btc_refund_locktime"] = json!(htlc.refund_locktime);
    r["target_amount"] = json!(target.to_string());
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

fn set_fundings(backend: &Backend, amounts: &[u64]) {
    backend.fundings.lock().unwrap().clear();
    for sats in amounts {
        backend.fund(*sats, Some(99));
    }
}

fn funding_of(backend: &Backend, input: &TxIn) -> u64 {
    let fundings = backend.fundings.lock().unwrap();
    fundings
        .iter()
        .find(|f| f.transaction.compute_txid() == input.previous_output.txid)
        .unwrap()
        .transaction
        .output[0]
        .value
        .to_sat()
}

#[tokio::test]
async fn dust_sent_to_the_htlc_does_not_block_a_claim_or_refund() {
    let amounts: Vec<u64> = std::iter::once(50_000)
        .chain(std::iter::repeat_n(330, 40))
        .collect();
    for refund in [false, true] {
        for with_history in [true, false] {
            let (client, _, backend, mut options) = spend_setup(with_history, refund, 50_000);
            set_fundings(&backend, &amounts);
            options.fee_rate = Some(FeeRate::from_sat_per_vb(5).unwrap());
            options.maximum_fee = Amount::from_sat(2_000);
            let receipt = if refund {
                client
                    .refund_bitcoin("bitcoin", &backend, options.clone())
                    .await
            } else {
                client
                    .claim_bitcoin("bitcoin", &backend, options.clone())
                    .await
            }
            .unwrap();
            assert_eq!(receipt.transaction.input.len(), 1);
            assert_eq!(funding_of(&backend, &receipt.transaction.input[0]), 50_000);
            if refund || !with_history {
                continue;
            }
            // The leftover sweep leaves the dust behind too.
            backend.confirm(receipt.transaction.compute_txid(), 100);
            backend.fund(20_000, Some(99));
            let swept = client
                .claim_bitcoin("bitcoin", &backend, options)
                .await
                .unwrap();
            assert_ne!(
                swept.transaction.compute_txid(),
                receipt.transaction.compute_txid()
            );
            assert_eq!(swept.transaction.input.len(), 1);
            assert_eq!(funding_of(&backend, &swept.transaction.input[0]), 20_000);
        }
    }
}

#[tokio::test]
async fn outputs_just_above_the_input_fee_do_not_push_a_spend_over_its_fee_cap() {
    // At 5 sat/vB an input costs under 500 sats, so each 600-sat output pays for
    // itself, but a second input takes the spend past the 1,000-sat cap.
    let amounts = [50_000, 600, 600, 600];
    for refund in [false, true] {
        for with_history in [true, false] {
            let (client, _, backend, mut options) = spend_setup(with_history, refund, 50_000);
            set_fundings(&backend, &amounts);
            options.fee_rate = Some(FeeRate::from_sat_per_vb(5).unwrap());
            options.maximum_fee = Amount::from_sat(1_000);
            let receipt = if refund {
                client
                    .refund_bitcoin("bitcoin", &backend, options.clone())
                    .await
            } else {
                client
                    .claim_bitcoin("bitcoin", &backend, options.clone())
                    .await
            }
            .unwrap();
            assert_eq!(receipt.transaction.input.len(), 1);
            assert_eq!(funding_of(&backend, &receipt.transaction.input[0]), 50_000);
        }
    }
}

#[tokio::test]
async fn uneconomical_top_up_is_kept_when_the_claim_needs_it() {
    let (client, _, backend, mut options) = spend_setup(true, false, 50_000);
    let mut amounts = vec![49_600, 600];
    amounts.extend([330; 5]);
    set_fundings(&backend, &amounts);
    // At 10 sat/vB a claim input costs about 980 sats, more than either small output.
    options.fee_rate = Some(FeeRate::from_sat_per_vb(10).unwrap());
    let receipt = client
        .claim_bitcoin("bitcoin", &backend, options)
        .await
        .unwrap();
    let mut spent: Vec<_> = receipt
        .transaction
        .input
        .iter()
        .map(|i| funding_of(&backend, i))
        .collect();
    spent.sort();
    assert_eq!(spent, [600, 49_600]);
}

#[tokio::test]
async fn fee_bump_keeps_inputs_that_became_uneconomical() {
    let (client, _, backend, mut options) = claim_setup(true);
    set_fundings(&backend, &[99_700, 400]);
    options.fee_rate = Some(FeeRate::from_sat_per_vb(2).unwrap());
    let first = client
        .claim_bitcoin("bitcoin", &backend, options.clone())
        .await
        .unwrap();
    assert_eq!(first.transaction.input.len(), 2);
    options.fee_rate = Some(FeeRate::from_sat_per_vb(10).unwrap());
    let bumped = client
        .claim_bitcoin("bitcoin", &backend, options)
        .await
        .unwrap();
    assert_ne!(
        bumped.transaction.compute_txid(),
        first.transaction.compute_txid()
    );
    let inputs = |tx: &Transaction| {
        tx.input
            .iter()
            .map(|i| i.previous_output)
            .collect::<Vec<_>>()
    };
    assert_eq!(inputs(&bumped.transaction), inputs(&first.transaction));
    assert_eq!(backend.submissions(), 2);
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
    let error = backend.observe(&address).await.unwrap_err();
    assert!(
        matches!(&error, Error::Transport(message) if message.contains("tip changed")),
        "{error:?}"
    );
    let wrong_network = EsploraBackend::new(&server.uri(), Network::Bitcoin).unwrap();
    assert!(wrong_network.observe(&address).await.is_err());
}

#[tokio::test]
async fn esplora_confirmed_funding_survives_mempool_spam() {
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (htlc, _, _, _, _) = fixture();
    let address = htlc.address().unwrap();
    let genesis = bitcoin::blockdata::constants::genesis_block(Network::Regtest);
    let hash = genesis.block_hash();
    let script_hash = bitcoin::hashes::sha256::Hash::hash(address.script_pubkey().as_bytes());
    // 0..50 are unconfirmed dust, 50..75 a full page of confirmed decoys, 75 the funding.
    let transactions: Vec<_> = (0..76)
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
                value: Amount::from_sat(if n == 75 { 50_000 } else { 330 }),
                script_pubkey: address.script_pubkey(),
            }],
        })
        .collect();
    let entry = |tx: &Transaction, confirmed: bool| {
        let status = if confirmed {
            json!({"confirmed":true,"block_height":0,"block_hash":hash,"block_time":genesis.header.time})
        } else {
            json!({"confirmed":false})
        };
        json!({"txid":tx.compute_txid(),"version":2,"locktime":tx.lock_time.to_consensus_u32(),"vin":[],"vout":[],"size":tx.total_size(),"weight":tx.weight().to_wu(),"fee":0,"status":status})
    };
    let mempool: Vec<_> = transactions[..50]
        .iter()
        .map(|tx| entry(tx, false))
        .collect();
    let decoys: Vec<_> = transactions[50..75]
        .iter()
        .map(|tx| entry(tx, true))
        .collect();
    let funding = vec![entry(&transactions[75], true)];
    let funding_txid = transactions[75].compute_txid();
    let last_decoy = transactions[74].compute_txid();
    let spender = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: funding_txid,
                vout: 0,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(49_000),
            script_pubkey: ScriptBuf::new(),
        }],
    };
    let spender_txid = spender.compute_txid();
    // Mempool on an electrum backend serves no /txs/chain; its /txs pages the whole
    // history, mempool first, ten at a time through after_txid.
    let listing: Vec<_> = mempool
        .iter()
        .chain(&decoys)
        .chain(&funding)
        .cloned()
        .collect();
    for electrum in [false, true] {
        // 0 unspent, 1 spent in the mempool, 2 spent in a block the listing missed.
        let spent = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
        let funding_spent = spent.clone();
        let ignores_cursor = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stale = ignores_cursor.clone();
        let (mempool, decoys, funding, listing) = (
            mempool.clone(),
            decoys.clone(),
            funding.clone(),
            listing.clone(),
        );
        let (transactions, spender) = (transactions.clone(), spender.clone());
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::method("GET"))
            .respond_with(move |request: &wiremock::Request| {
                let path = request.url.path();
                if path == "/block-height/0" || path == "/blocks/tip/hash" {
                    return ResponseTemplate::new(200).set_body_string(hash.to_string());
                }
                if path == "/blocks/tip/height" {
                    return ResponseTemplate::new(200).set_body_string("0");
                }
                if path == format!("/block/{hash}/header") {
                    return ResponseTemplate::new(200).set_body_string(
                        bitcoin::consensus::encode::serialize_hex(&genesis.header),
                    );
                }
                if electrum && path == format!("/scripthash/{script_hash:x}/txs") {
                    let after = request
                        .url
                        .query_pairs()
                        .find(|(key, _)| key == "after_txid")
                        .map(|(_, txid)| txid.into_owned())
                        .filter(|_| !stale.load(std::sync::atomic::Ordering::Relaxed));
                    let start = after
                        .and_then(|txid| listing.iter().position(|e| e["txid"] == txid))
                        .map_or(0, |position| position + 1);
                    let end = listing.len().min(start + 10);
                    return ResponseTemplate::new(200).set_body_json(&listing[start..end]);
                }
                if !electrum {
                    if path == format!("/scripthash/{script_hash:x}/txs") {
                        return ResponseTemplate::new(200).set_body_json(&mempool);
                    }
                    if path == format!("/scripthash/{script_hash:x}/txs/chain") {
                        return ResponseTemplate::new(200).set_body_json(&decoys);
                    }
                    if path == format!("/scripthash/{script_hash:x}/txs/chain/{last_decoy}") {
                        return ResponseTemplate::new(200).set_body_json(&funding);
                    }
                }
                if path == format!("/tx/{spender_txid}/raw") {
                    return ResponseTemplate::new(200)
                        .set_body_bytes(bitcoin::consensus::serialize(&spender));
                }
                let state = funding_spent.load(std::sync::atomic::Ordering::Relaxed);
                if path == format!("/tx/{funding_txid}/outspend/0") && state > 0 {
                    return ResponseTemplate::new(200).set_body_json(json!({
                        "spent": true,
                        "txid": spender_txid,
                        "vin": 0,
                        "status": if state == 1 {
                            json!({"confirmed": false})
                        } else {
                            json!({"confirmed": true, "block_height": 0, "block_hash": hash})
                        }
                    }));
                }
                for tx in &transactions {
                    if path == format!("/tx/{}/raw", tx.compute_txid()) {
                        return ResponseTemplate::new(200)
                            .set_body_bytes(bitcoin::consensus::serialize(tx));
                    }
                    if path == format!("/tx/{}/outspend/0", tx.compute_txid()) {
                        return ResponseTemplate::new(200).set_body_json(json!({"spent":false}));
                    }
                }
                ResponseTemplate::new(404)
            })
            .mount(&server)
            .await;
        let backend = EsploraBackend::new(&server.uri(), Network::Regtest).unwrap();
        // The same spam must not hide our pending spend of the funding either.
        spent.store(1, std::sync::atomic::Ordering::Relaxed);
        let history = backend.history(&address).await.unwrap();
        assert_eq!(history.observation.utxos.len(), 75, "electrum {electrum}");
        let pending = history
            .transactions
            .iter()
            .find(|entry| entry.transaction.compute_txid() == spender_txid)
            .expect("pending spend hidden by mempool spam");
        assert_eq!(pending.confirmed_height, None);
        spent.store(2, std::sync::atomic::Ordering::Relaxed);
        let error = backend.history(&address).await.unwrap_err();
        assert!(
            matches!(&error, Error::Transport(message) if message.contains("changed during observation")),
            "{error:?}"
        );
        spent.store(0, std::sync::atomic::Ordering::Relaxed);
        let observed = backend.observe(&address).await.unwrap();
        assert_eq!(observed.utxos.len(), 76, "electrum {electrum}");
        let funded = observed
            .utxos
            .iter()
            .find(|utxo| utxo.outpoint.txid == funding_txid)
            .expect("confirmed funding hidden by mempool spam");
        assert_eq!(funded.confirmed_height, Some(0));
        assert_eq!(funded.output.value, Amount::from_sat(50_000));
        if electrum {
            // A server that ignores the cursor would repeat the first page forever.
            ignores_cursor.store(true, std::sync::atomic::Ordering::Relaxed);
            let error = backend.observe(&address).await.unwrap_err();
            assert!(error.to_string().contains("did not advance"), "{error}");
        }
    }
}

/// Line-delimited JSON-RPC server for `connections` sequential sockets. It answers the
/// genesis header itself; `respond` gets the connection index and method and returns
/// `None` to drop the socket, `Ok` for a result or `Err` for a JSON-RPC error.
#[cfg(feature = "electrum")]
fn electrum_mock(
    connections: usize,
    respond: impl Fn(usize, &str) -> Option<std::result::Result<serde_json::Value, serde_json::Value>>
    + Send
    + 'static,
) -> (String, std::thread::JoinHandle<()>) {
    use serde_json::Value;
    use serde_json::json;
    use std::io::BufRead;
    use std::io::BufReader;
    use std::io::Write;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("tcp://{}", listener.local_addr().unwrap());
    let header = bitcoin::consensus::encode::serialize_hex(
        &bitcoin::blockdata::constants::genesis_block(Network::Regtest).header,
    );
    let server = std::thread::spawn(move || {
        for connection in 0..connections {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let request: Value = serde_json::from_str(&line).unwrap();
                let method = request["method"].as_str().unwrap();
                let reply = if method == "blockchain.block.header" {
                    Some(Ok(json!(header)))
                } else {
                    respond(connection, method)
                };
                let body = match reply {
                    Some(Ok(result)) => json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
                    Some(Err(error)) => json!({"jsonrpc":"2.0","id":request["id"],"error":error}),
                    None => break,
                };
                writeln!(stream, "{body}").unwrap();
            }
        }
    });
    (url, server)
}

#[cfg(feature = "electrum")]
fn electrum_test_transaction(address: &Address) -> Transaction {
    Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn::default()],
        output: vec![TxOut {
            value: Amount::from_sat(1000),
            script_pubkey: address.script_pubkey(),
        }],
    }
}

#[cfg(feature = "electrum")]
#[tokio::test]
async fn electrum_reads_actual_funding_outputs_and_converts_btc_per_kb_fees() {
    use serde_json::json;
    let (htlc, _, _, _, _) = fixture();
    let address = htlc.address().unwrap();
    let tx = electrum_test_transaction(&address);
    let header = bitcoin::consensus::encode::serialize_hex(
        &bitcoin::blockdata::constants::genesis_block(Network::Regtest).header,
    );
    let (url, server) = electrum_mock(1, move |_, method| {
        Some(Ok(match method {
            "blockchain.headers.subscribe" => json!({"height":0,"hex":header}),
            "blockchain.scripthash.listunspent" => {
                json!([{"tx_hash":tx.compute_txid(),"tx_pos":0,"value":1000,"height":0}])
            }
            "blockchain.transaction.get" => {
                json!(bitcoin::consensus::encode::serialize_hex(&tx))
            }
            "blockchain.estimatefee" => json!(0.00002),
            method => panic!("unexpected method {method}"),
        }))
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

#[cfg(feature = "electrum")]
#[tokio::test]
async fn electrum_reports_a_tip_change_during_observation_as_retryable() {
    use serde_json::json;
    let (htlc, _, _, _, _) = fixture();
    let genesis = bitcoin::blockdata::constants::genesis_block(Network::Regtest).header;
    let mut next = genesis;
    next.nonce += 1;
    let subscriptions = std::sync::atomic::AtomicUsize::new(0);
    let (url, server) = electrum_mock(1, move |_, method| {
        Some(Ok(match method {
            "blockchain.headers.subscribe" => {
                let first = subscriptions.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0;
                let header = if first { genesis } else { next };
                json!({"height":0,"hex":bitcoin::consensus::encode::serialize_hex(&header)})
            }
            "blockchain.scripthash.listunspent" => json!([]),
            method => panic!("unexpected method {method}"),
        }))
    });
    let backend = ElectrumBackend::connect(url, Network::Regtest, 3)
        .await
        .unwrap();
    let error = backend.observe(&htlc.address().unwrap()).await.unwrap_err();
    assert!(
        matches!(&error, Error::Transport(message) if message.contains("tip changed")),
        "{error:?}"
    );
    drop(backend);
    server.join().unwrap();
}

#[cfg(feature = "electrum")]
#[tokio::test]
async fn electrum_reports_unknown_transactions_as_missing() {
    use serde_json::json;
    let missing = [
        json!({"code":-5,"message":"No such mempool or blockchain transaction"}),
        json!({
            "code": 2,
            "message": "daemon error: DaemonError({'code': -5, 'message': 'No such mempool or blockchain transaction. Use gettransaction for wallet transactions.'})"
        }),
        json!("missing transaction"),
    ];
    for error in missing {
        let reply = error.clone();
        let (url, server) = electrum_mock(1, move |_, method| {
            assert_eq!(method, "blockchain.transaction.get");
            Some(Err(reply.clone()))
        });
        let backend = ElectrumBackend::connect(url, Network::Regtest, 3)
            .await
            .unwrap();
        let result = backend.transaction(Txid::all_zeros()).await;
        assert!(matches!(result, Ok(None)), "{error} gave {result:?}");
        drop(backend);
        server.join().unwrap();
    }
    let (url, server) = electrum_mock(1, |_, _| {
        Some(Err(
            json!({"code":-101,"message":"excessive resource usage"}),
        ))
    });
    let backend = ElectrumBackend::connect(url, Network::Regtest, 3)
        .await
        .unwrap();
    assert!(backend.transaction(Txid::all_zeros()).await.is_err());
    drop(backend);
    server.join().unwrap();
}

#[cfg(feature = "electrum")]
#[tokio::test]
async fn electrum_reconnects_after_the_socket_drops() {
    use serde_json::json;
    let (htlc, _, _, _, _) = fixture();
    let tx = electrum_test_transaction(&htlc.address().unwrap());
    let id = tx.compute_txid();
    let (url, server) = electrum_mock(2, move |connection, method| {
        assert_eq!(method, "blockchain.transaction.get");
        (connection == 1).then(|| Ok(json!(bitcoin::consensus::encode::serialize_hex(&tx))))
    });
    let backend = ElectrumBackend::connect(url, Network::Regtest, 3)
        .await
        .unwrap();
    assert_eq!(
        backend
            .transaction(id)
            .await
            .unwrap()
            .unwrap()
            .compute_txid(),
        id
    );
    drop(backend);
    server.join().unwrap();
}

fn record_claim_destination(
    storage: &crate::InMemorySwapStorage,
    local: Option<&str>,
    server: &str,
) {
    use crate::SwapStorage;
    let mut stored = storage.get_swap("bitcoin").unwrap().unwrap();
    let GetSwapResponse::EvmToBitcoin(response) = &mut stored.response else {
        panic!("expected an EVM to Bitcoin swap");
    };
    response.target_btc_address = Some(server.into());
    stored.metadata.target_address = local.map(str::to_owned);
    storage.store_swap(stored).unwrap();
}

fn claim_config(
    backend: std::sync::Arc<Backend>,
    options: &BitcoinSpendOptions,
) -> crate::tracker::AutoClaimConfig {
    crate::tracker::AutoClaimConfig {
        bitcoin: Some(crate::tracker::BitcoinAutoClaim {
            backend,
            maximum_fee: options.maximum_fee,
            minimum_remaining_blocks: options.minimum_remaining_blocks,
            confirmation_target: options.confirmation_target,
        }),
        ..Default::default()
    }
}

fn auto_claimer(
    client: Client,
    backend: std::sync::Arc<Backend>,
    options: &BitcoinSpendOptions,
) -> crate::tracker::AutoClaimer {
    crate::tracker::AutoClaimer::new(client, claim_config(backend, options)).unwrap()
}

async fn auto_claim(
    claimer: &crate::tracker::AutoClaimer,
) -> Result<crate::tracker::ExecutionResult> {
    use crate::tracker::ActionExecutor;
    claimer
        .execute("bitcoin", &crate::actions::ActionKind::Claim)
        .await
}

#[tokio::test]
async fn auto_claim_pays_only_the_recorded_bitcoin_destination() {
    use crate::tracker::ExecutionResult;
    let (client, storage, backend, options) = claim_setup(true);
    let backend = std::sync::Arc::new(backend);
    let claimer = auto_claimer(client, backend.clone(), &options);
    // The server's address alone is not enough.
    record_claim_destination(&storage, None, &options.destination);
    let Ok(ExecutionResult::NotSent(error)) = auto_claim(&claimer).await else {
        panic!("a claim without a recorded destination is refused");
    };
    assert!(error.to_string().contains("destination"), "{error}");
    assert_eq!(backend.submissions(), 0);
    record_claim_destination(&storage, Some(&options.destination), &options.destination);
    let Ok(ExecutionResult::Submitted { reference }) = auto_claim(&claimer).await else {
        panic!("the claim is broadcast");
    };
    assert_eq!(
        reference,
        backend.broadcasts.lock().unwrap()[0]
            .compute_txid()
            .to_string()
    );
    assert_eq!(journaled(&storage).0, Some(reference));
}

#[tokio::test]
async fn auto_claim_asks_for_a_destination_only_when_none_was_recorded() {
    use crate::tracker::ExecutionResult;
    let secp = Secp256k1::new();
    let key = Keypair::from_secret_key(&secp, &SecretKey::from_slice(&[7; 32]).unwrap());
    let other = Address::p2tr(&secp, key.x_only_public_key().0, None, Network::Regtest);
    for recorded in [false, true] {
        let (client, storage, backend, options) = claim_setup(true);
        let backend = std::sync::Arc::new(backend);
        let destination = Address::from_str(&options.destination)
            .unwrap()
            .assume_checked();
        let answer = if recorded {
            record_claim_destination(&storage, Some(&options.destination), &options.destination);
            other.to_string()
        } else {
            record_claim_destination(&storage, None, &other.to_string());
            options.destination.clone()
        };
        let claimer = crate::tracker::AutoClaimer::new(
            client,
            crate::tracker::AutoClaimConfig {
                destination: Some(std::sync::Arc::new(move |_: &crate::StoredSwap| {
                    Some(answer.clone())
                })),
                ..claim_config(backend.clone(), &options)
            },
        )
        .unwrap();
        let Ok(ExecutionResult::Submitted { .. }) = auto_claim(&claimer).await else {
            panic!("recorded {recorded}: the claim is broadcast");
        };
        assert_eq!(
            backend.broadcasts.lock().unwrap()[0].output[0].script_pubkey,
            destination.script_pubkey(),
            "recorded {recorded}"
        );
    }
}

#[tokio::test]
async fn auto_claim_retries_a_refused_bitcoin_spend_but_not_a_failed_broadcast() {
    use crate::tracker::ExecutionResult;
    let (client, storage, backend, mut options) = claim_setup(true);
    let backend = std::sync::Arc::new(backend);
    record_claim_destination(&storage, Some(&options.destination), &options.destination);
    let generous = options.maximum_fee;
    options.maximum_fee = Amount::from_sat(1);
    let claimer = auto_claimer(client.clone(), backend.clone(), &options);
    let Ok(ExecutionResult::NotSent(error)) = auto_claim(&claimer).await else {
        panic!("a spend over the fee cap is refused before it is journaled");
    };
    assert!(error.to_string().contains("fee"), "{error}");
    assert_eq!(journaled(&storage).0, None);
    options.maximum_fee = generous;
    backend
        .reject
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let claimer = auto_claimer(client, backend.clone(), &options);
    assert!(auto_claim(&claimer).await.is_err());
    assert!(journaled(&storage).0.is_some());
    // The journaled spend may have reached the network, so it is not rebuilt.
    backend
        .reject
        .store(false, std::sync::atomic::Ordering::Relaxed);
    assert!(auto_claim(&claimer).await.is_err());
    assert_eq!(backend.submissions(), 0);
}

/// The claim fixture with its only funding still in the mempool.
fn mempool_claim_setup() -> (
    Client,
    std::sync::Arc<crate::InMemorySwapStorage>,
    std::sync::Arc<Backend>,
    BitcoinSpendOptions,
) {
    let (client, storage, backend, options) = claim_setup(true);
    backend.fundings.lock().unwrap().clear();
    backend.fund(100_000, None);
    (client, storage, std::sync::Arc::new(backend), options)
}

#[tokio::test]
async fn a_claim_at_zero_confirmations_spends_a_mempool_funding() {
    let (client, _, backend, mut options) = mempool_claim_setup();
    options.minimum_confirmations = 1;
    assert!(
        client
            .claim_bitcoin("bitcoin", backend.as_ref(), options.clone())
            .await
            .is_err()
    );
    assert_eq!(backend.submissions(), 0);
    options.minimum_confirmations = 0;
    let receipt = client
        .claim_bitcoin("bitcoin", backend.as_ref(), options)
        .await
        .unwrap();
    assert_eq!(
        receipt.transaction.input[0].previous_output.txid,
        backend.fundings.lock().unwrap()[0]
            .transaction
            .compute_txid()
    );
    assert_eq!(backend.submissions(), 1);
}

#[tokio::test]
async fn auto_claim_follows_the_client_depth_and_claims_a_mempool_funding_by_default() {
    use crate::tracker::ExecutionResult;
    let (client, storage, backend, options) = mempool_claim_setup();
    record_claim_destination(&storage, Some(&options.destination), &options.destination);
    let claimer = auto_claimer(client.clone(), backend.clone(), &options);
    client.set_bitcoin_min_confirmations(Some(1));
    let Ok(ExecutionResult::NotSent(_)) = auto_claim(&claimer).await else {
        panic!("a funding below the client's depth is not claimed");
    };
    assert_eq!(backend.submissions(), 0);
    client.set_bitcoin_min_confirmations(None);
    let Ok(ExecutionResult::Submitted { .. }) = auto_claim(&claimer).await else {
        panic!("the default depth of 0 claims the mempool funding");
    };
    assert_eq!(backend.submissions(), 1);
}

#[tokio::test]
async fn auto_claim_uses_the_depth_the_tracker_read_the_server_leg_at() {
    use crate::SwapStorage;
    use crate::observation::ContractRef;
    use crate::observation::SwapContracts;
    use crate::tracker::ExecutionResult;
    use crate::tracker::TrackingRecord;
    let (client, storage, backend, options) = mempool_claim_setup();
    record_claim_destination(&storage, Some(&options.destination), &options.destination);
    client.set_bitcoin_min_confirmations(Some(3));
    let journal = |version| {
        let record = TrackingRecord {
            version,
            enabled: true,
            contracts: SwapContracts {
                client: None,
                server: Some(ContractRef::Bitcoin {
                    htlc: client.bitcoin_htlc("bitcoin").unwrap(),
                    expected_sats: 100_000,
                    confirmations: Some(0),
                }),
            },
            options: Default::default(),
            attempt: None,
            completed_ms: None,
            server_status: None,
        };
        storage
            .update_metadata(
                "bitcoin",
                SwapMetadata {
                    tracking: Some(serde_json::to_value(record).unwrap()),
                    ..Default::default()
                },
            )
            .unwrap();
    };
    let claimer = auto_claimer(client.clone(), backend.clone(), &options);
    // A version 1 journal pinned every server leg, so its depth is not the tracker's.
    journal(1);
    let Ok(ExecutionResult::NotSent(_)) = auto_claim(&claimer).await else {
        panic!("an old journal's server leg follows the client's depth");
    };
    assert_eq!(backend.submissions(), 0);
    journal(2);
    let Ok(ExecutionResult::Submitted { .. }) = auto_claim(&claimer).await else {
        panic!("the depth the reference carries is the one the tracker read at");
    };
    assert_eq!(backend.submissions(), 1);
}

#[cfg(feature = "gasless")]
#[test]
fn contract_references_leave_the_server_bitcoin_depth_to_the_client() {
    use crate::observation::ContractRef;
    use crate::observation::ReferenceOptions;
    let options = ReferenceOptions {
        client_confirmations: 3,
        server_confirmations: 0,
        evm_source_sender: Some(crate::evm::Address::repeat_byte(1)),
        evm_lock_token: Some(crate::evm::Address::repeat_byte(2)),
        ..Default::default()
    };
    let bitcoin_depth = |client_funds: bool| {
        use crate::SwapStorage;
        let (client, storage, _, _) = spend_setup(true, client_funds, 100_000);
        let mut stored = storage.get_swap("bitcoin").unwrap().unwrap();
        let mut response = serde_json::to_value(&stored.response).unwrap();
        for field in ["server_evm_address", "evm_htlc_address", "wbtc_address"] {
            response[field] = format!("{:#x}", crate::evm::Address::repeat_byte(3)).into();
        }
        stored.response = serde_json::from_value(response).unwrap();
        storage.store_swap(stored).unwrap();
        let contracts = client.contract_references("bitcoin", &options).unwrap();
        let leg = if client_funds {
            contracts.client
        } else {
            contracts.server
        };
        let Some(ContractRef::Bitcoin { confirmations, .. }) = leg else {
            panic!("expected a Bitcoin leg");
        };
        confirmations
    };
    assert_eq!(bitcoin_depth(false), None);
    assert_eq!(bitcoin_depth(true), Some(3));
}
