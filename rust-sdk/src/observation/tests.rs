use super::*;
use crate::bitcoin::BitcoinHistory;
use crate::bitcoin::BitcoinHistoryTransaction;
use crate::bitcoin::BitcoinHtlc;
use crate::bitcoin::BitcoinObservation;
use crate::bitcoin::BitcoinUtxo;
use bitcoin::Amount;
use bitcoin::OutPoint;
use bitcoin::ScriptBuf;
use bitcoin::Sequence;
use bitcoin::Transaction;
use bitcoin::TxIn;
use bitcoin::TxOut;
use bitcoin::Witness;
use bitcoin::absolute;
use bitcoin::secp256k1::Keypair;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::secp256k1::SecretKey;
use bitcoin::taproot::LeafVersion;
use bitcoin::transaction;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
fn key(n: u8) -> bitcoin::XOnlyPublicKey {
    Keypair::from_secret_key(&Secp256k1::new(), &SecretKey::from_slice(&[n; 32]).unwrap())
        .x_only_public_key()
        .0
}
fn htlc() -> BitcoinHtlc {
    BitcoinHtlc {
        network: Network::Regtest,
        hash_lock: hash160::Hash::hash(&[42; 32]).to_byte_array(),
        claimer: key(1),
        refunder: key(2),
        refund_locktime: 100,
        script_version: 1,
    }
}
fn funding(htlc: &BitcoinHtlc) -> BitcoinHistory {
    let transaction = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(10000),
            script_pubkey: htlc.address().unwrap().script_pubkey(),
        }],
    };
    let utxo = BitcoinUtxo {
        outpoint: OutPoint {
            txid: transaction.compute_txid(),
            vout: 0,
        },
        output: transaction.output[0].clone(),
        confirmed_height: Some(99),
    };
    BitcoinHistory {
        observation: BitcoinObservation {
            tip: bitcoin::BlockHash::all_zeros(),
            height: 100,
            median_time_past: 1000,
            utxos: vec![utxo],
        },
        transactions: vec![BitcoinHistoryTransaction {
            transaction,
            confirmed_height: Some(99),
        }],
    }
}
#[test]
fn bitcoin_confirmations_preimages_spend_paths_and_reorgs() {
    let h = htlc();
    let mut history = funding(&h);
    assert_eq!(
        bitcoin_reader::classify(&h, 10000, 2, &history)
            .unwrap()
            .state,
        HtlcState::Confirmed
    );
    assert_eq!(
        bitcoin_reader::classify(&h, 10000, 3, &history)
            .unwrap()
            .state,
        HtlcState::Mempool
    );
    assert_eq!(
        bitcoin_reader::classify(&h, 10001, 1, &history)
            .unwrap()
            .state,
        HtlcState::Invalid
    );
    let scripts = h.scripts().unwrap();
    let control = scripts
        .spend_info
        .control_block(&(scripts.claim.clone(), LeafVersion::TapScript))
        .unwrap();
    let outpoint = history.observation.utxos[0].outpoint;
    let spend = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: outpoint,
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::from_slice(&[
                vec![1; 64],
                vec![42; 32],
                scripts.claim.as_bytes().to_vec(),
                control.serialize(),
            ]),
        }],
        output: vec![],
    };
    history.observation.utxos.clear();
    history.transactions.push(BitcoinHistoryTransaction {
        transaction: spend.clone(),
        confirmed_height: Some(100),
    });
    let observation = bitcoin_reader::classify(&h, 10000, 1, &history).unwrap();
    assert_eq!(observation.state, HtlcState::SpentClaim);
    assert_eq!(observation.preimage.unwrap().expose(), &[42; 32]);
    // The depth gates the funding only, so a shallower spend still counts.
    assert_eq!(
        bitcoin_reader::classify(&h, 10000, 2, &history)
            .unwrap()
            .state,
        HtlcState::SpentClaim
    );
    history.transactions[1].transaction.input[0].witness = Witness::from_slice(&[
        vec![1; 64],
        vec![41; 32],
        scripts.claim.as_bytes().to_vec(),
        control.serialize(),
    ]);
    assert_eq!(
        bitcoin_reader::classify(&h, 10000, 1, &history)
            .unwrap()
            .state,
        HtlcState::SpentUnknown
    );
    let control = scripts
        .spend_info
        .control_block(&(scripts.refund.clone(), LeafVersion::TapScript))
        .unwrap();
    history.transactions[1].transaction.input[0].witness = Witness::from_slice(&[
        vec![1; 64],
        scripts.refund.as_bytes().to_vec(),
        control.serialize(),
    ]);
    assert_eq!(
        bitcoin_reader::classify(&h, 10000, 1, &history)
            .unwrap()
            .state,
        HtlcState::SpentRefund
    );
    // Reorg restores the original UTXO: there is no terminal-state latch.
    assert_eq!(
        bitcoin_reader::classify(&h, 10000, 1, &funding(&h))
            .unwrap()
            .state,
        HtlcState::Confirmed
    );
}
#[test]
fn bitcoin_surplus_after_a_spend_keeps_the_spent_verdict() {
    let h = htlc();
    let mut history = funding(&h);
    let scripts = h.scripts().unwrap();
    let control = scripts
        .spend_info
        .control_block(&(scripts.claim.clone(), LeafVersion::TapScript))
        .unwrap();
    let outpoint = history.observation.utxos[0].outpoint;
    let spend = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: outpoint,
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::from_slice(&[
                vec![1; 64],
                vec![42; 32],
                scripts.claim.as_bytes().to_vec(),
                control.serialize(),
            ]),
        }],
        output: vec![],
    };
    history.observation.utxos.clear();
    history.transactions.push(BitcoinHistoryTransaction {
        transaction: spend,
        confirmed_height: Some(100),
    });
    let surplus = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(330),
            script_pubkey: h.address().unwrap().script_pubkey(),
        }],
    };
    history.observation.utxos.push(BitcoinUtxo {
        outpoint: OutPoint {
            txid: surplus.compute_txid(),
            vout: 0,
        },
        output: surplus.output[0].clone(),
        confirmed_height: Some(100),
    });
    history.transactions.push(BitcoinHistoryTransaction {
        transaction: surplus,
        confirmed_height: Some(100),
    });
    let observation = bitcoin_reader::classify(&h, 10000, 1, &history).unwrap();
    assert_eq!(observation.state, HtlcState::SpentClaim);
    assert_eq!(observation.preimage.unwrap().expose(), &[42; 32]);
    assert_eq!(observation.remaining_amount, "330");
    assert_eq!(observation.funded_amount, "10330");
    history.transactions[1].confirmed_height = None;
    assert_eq!(
        bitcoin_reader::classify(&h, 10000, 1, &history)
            .unwrap()
            .state,
        HtlcState::SpentClaim
    );
}
#[test]
fn arkade_classification_binds_the_spending_input_and_executed_leaf() {
    let options = crate::arkade::StrictVhtlcOptions {
        sender: key(1),
        receiver: key(2),
        server: key(3),
        preimage_hash: bitcoin::hashes::ripemd160::Hash::from_byte_array(
            hash160::Hash::hash(&[42; 32]).to_byte_array(),
        ),
        refund_locktime: 100,
        unilateral_claim_delay: Sequence::from_height(10),
        unilateral_refund_delay: Sequence::from_height(20),
        unilateral_refund_without_receiver_delay: Sequence::from_height(30),
    };
    let outpoint = OutPoint {
        txid: bitcoin::Txid::from_byte_array([7; 32]),
        vout: 0,
    };
    let mut psbt = bitcoin::Psbt::from_unsigned_tx(Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: outpoint,
            ..Default::default()
        }],
        output: vec![],
    })
    .unwrap();
    let tree = crate::arkade::StrictVhtlcScript::new(options.clone(), Network::Regtest).unwrap();
    let script = options.claim_script();
    let control = tree
        .taproot_spend_info()
        .control_block(&(script.clone(), LeafVersion::TapScript))
        .unwrap();
    psbt.inputs[0].final_script_witness = Some(Witness::from_slice(&[
        vec![1; 64],
        vec![1; 64],
        vec![42; 32],
        script.into_bytes(),
        control.serialize(),
    ]));
    assert_eq!(
        arkade_reader::classify_spend(&psbt, outpoint, &options)
            .unwrap()
            .0,
        HtlcState::SpentClaim
    );
    assert_eq!(
        arkade_reader::classify_spend(&psbt, OutPoint::null(), &options)
            .unwrap()
            .0,
        HtlcState::SpentUnknown
    );
    let refund = options.refund_script();
    let control = tree
        .taproot_spend_info()
        .control_block(&(refund.clone(), LeafVersion::TapScript))
        .unwrap();
    // A valid secret inserted into a refund witness must not change its path.
    psbt.inputs[0].final_script_witness = Some(Witness::from_slice(&[
        vec![42; 32],
        refund.into_bytes(),
        control.serialize(),
    ]));
    assert_eq!(
        arkade_reader::classify_spend(&psbt, outpoint, &options)
            .unwrap()
            .0,
        HtlcState::SpentRefund
    );
}
struct Reader {
    fail: AtomicBool,
    spent: AtomicBool,
}
impl ContractReader for Reader {
    fn can_observe(&self, reference: &ContractRef) -> bool {
        matches!(reference, ContractRef::Bitcoin { .. })
    }
    fn observe<'a>(&'a self, _: &'a ContractRef) -> ObservationFuture<'a, Observation> {
        Box::pin(async move {
            if self.fail.load(Ordering::Relaxed) {
                return Err(Error::Transport("offline".into()));
            }
            Ok(Observation {
                state: if self.spent.load(Ordering::Relaxed) {
                    HtlcState::SpentClaim
                } else {
                    HtlcState::Confirmed
                },
                point: None,
                funded_amount: "10000".into(),
                remaining_amount: "10000".into(),
                preimage: None,
            })
        })
    }
}
#[tokio::test]
async fn manager_invalidates_stale_and_failed_reads_and_allows_reorg_regression() {
    let reader = Arc::new(Reader {
        fail: AtomicBool::new(false),
        spent: AtomicBool::new(true),
    });
    let manager = ContractManager::new(
        reader.clone(),
        ManagerOptions {
            maximum_age: Duration::from_millis(15),
            ..Default::default()
        },
    )
    .unwrap();
    let reference = ContractRef::Bitcoin {
        htlc: htlc(),
        expected_sats: 10000,
        confirmations: Some(1),
    };
    let key = reference.key().unwrap();
    manager.register(reference).await.unwrap();
    assert_eq!(
        manager.get(&key).unwrap().unwrap().state,
        HtlcState::SpentClaim
    );
    reader.spent.store(false, Ordering::Relaxed);
    manager.reconcile(&key).await.unwrap();
    assert_eq!(
        manager.get(&key).unwrap().unwrap().state,
        HtlcState::Confirmed
    );
    reader.fail.store(true, Ordering::Relaxed);
    assert!(manager.reconcile(&key).await.is_err());
    assert_eq!(
        manager.get(&key).unwrap().unwrap().state,
        HtlcState::Unknown
    );
    reader.fail.store(false, Ordering::Relaxed);
    manager.reconcile(&key).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        manager.get(&key).unwrap().unwrap().state,
        HtlcState::Unknown
    );
    let monitor = manager.start().unwrap();
    monitor.shutdown().await.unwrap();
    manager.unregister(&key).unwrap();
    assert!(manager.get(&key).unwrap().is_none());
}
struct SlowReader {
    calls: AtomicUsize,
}
impl ContractReader for SlowReader {
    fn can_observe(&self, reference: &ContractRef) -> bool {
        matches!(reference, ContractRef::Bitcoin { .. })
    }
    fn observe<'a>(&'a self, _: &'a ContractRef) -> ObservationFuture<'a, Observation> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(Observation {
                state: HtlcState::Confirmed,
                point: None,
                funded_amount: "10000".into(),
                remaining_amount: "10000".into(),
                preimage: None,
            })
        })
    }
}
#[tokio::test]
async fn manager_lock_wait_does_not_consume_read_budget() {
    let reader = Arc::new(SlowReader {
        calls: AtomicUsize::new(0),
    });
    let manager = ContractManager::new(
        reader.clone(),
        ManagerOptions {
            read_timeout: Duration::from_millis(150),
            ..Default::default()
        },
    )
    .unwrap();
    let reference = ContractRef::Bitcoin {
        htlc: htlc(),
        expected_sats: 10000,
        confirmations: Some(1),
    };
    let key = reference.key().unwrap();
    manager.register_unread(reference).unwrap();
    let a = manager.clone();
    let b = manager.clone();
    // B queues behind A's 100ms read; its own 150ms budget must start once it
    // holds the lock, not while it waits.
    let (ra, rb) = tokio::join!(a.reconcile(&key), async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        b.reconcile(&key).await
    });
    ra.unwrap();
    rb.unwrap();
    assert_eq!(
        manager.get(&key).unwrap().unwrap().state,
        HtlcState::Confirmed
    );
    assert_eq!(reader.calls.load(Ordering::Relaxed), 2);
}
#[test]
fn timelocks_use_chain_height_and_mtp_and_preimages_are_redacted() {
    let point = ChainPoint {
        height: 100,
        hash: String::new(),
        timestamp: 500_000_001,
    };
    assert!(Timelock::bitcoin(100).expired(&point));
    assert!(!Timelock::bitcoin(101).expired(&point));
    assert!(!Timelock::bitcoin(500_000_001).expired(&point));
    assert!(Timelock::Timestamp(500_000_001).expired(&point));
    let lock = HashLock::Sha256(sha256::Hash::hash(&[42; 32]).to_byte_array());
    let secret = lock.preimage(&[42; 32]).unwrap();
    assert!(!format!("{secret:?}").contains("42"));
    assert!(lock.preimage(&[42; 31]).is_none());
}
/// Serves one fixed address history.
struct History(BitcoinHistory);
impl crate::bitcoin::BitcoinBackend for History {
    fn network(&self) -> Network {
        Network::Regtest
    }
    fn history<'a>(
        &'a self,
        _: &'a bitcoin::Address,
    ) -> crate::bitcoin::ChainFuture<'a, BitcoinHistory> {
        Box::pin(async { Ok(self.0.clone()) })
    }
    fn observe<'a>(
        &'a self,
        _: &'a bitcoin::Address,
    ) -> crate::bitcoin::ChainFuture<'a, BitcoinObservation> {
        Box::pin(async { Ok(self.0.observation.clone()) })
    }
    fn transaction(
        &self,
        _: bitcoin::Txid,
    ) -> crate::bitcoin::ChainFuture<'_, Option<Transaction>> {
        Box::pin(async { Err(Error::Transport("unused".into())) })
    }
    fn fee_rate(&self, _: u16) -> crate::bitcoin::ChainFuture<'_, bitcoin::FeeRate> {
        Box::pin(async { Err(Error::Transport("unused".into())) })
    }
    fn broadcast<'a>(
        &'a self,
        _: &'a Transaction,
    ) -> crate::bitcoin::ChainFuture<'a, bitcoin::Txid> {
        Box::pin(async { Err(Error::Transport("unused".into())) })
    }
}
#[tokio::test]
async fn a_bitcoin_reference_without_a_depth_follows_the_client_on_every_read() {
    let h = htlc();
    let mut history = funding(&h);
    history.transactions[0].confirmed_height = None;
    history.observation.utxos[0].confirmed_height = None;
    let manager = ContractManager::new(
        Arc::new(BitcoinReader::new(Arc::new(History(history)))),
        ManagerOptions::default(),
    )
    .unwrap();
    let client = crate::Client::new("http://127.0.0.1:1").unwrap();
    let state = |confirmations| {
        let manager = manager.clone();
        async move {
            let reference = ContractRef::Bitcoin {
                htlc: htlc(),
                expected_sats: 10000,
                confirmations,
            };
            let key = reference.key().unwrap();
            manager.unregister(&key).unwrap();
            manager.register(reference).await.unwrap();
            manager.get(&key).unwrap().unwrap().state
        }
    };
    client.set_bitcoin_min_confirmations(Some(1));
    assert_eq!(state(None).await, HtlcState::Confirmed, "not yet following");
    manager.follow_bitcoin_min_confirmations(&client).unwrap();
    assert_eq!(state(None).await, HtlcState::Mempool);
    client.set_bitcoin_min_confirmations(None);
    assert_eq!(state(None).await, HtlcState::Confirmed);
    // A reference's own depth overrides the client's.
    assert_eq!(state(Some(1)).await, HtlcState::Mempool);
    client.set_bitcoin_min_confirmations(Some(2));
    assert_eq!(state(Some(0)).await, HtlcState::Confirmed);
    // One manager cannot serve two clients' depths.
    assert!(
        manager
            .follow_bitcoin_min_confirmations(&client.clone())
            .is_ok()
    );
    let other = crate::Client::new("http://127.0.0.1:1").unwrap();
    assert!(manager.follow_bitcoin_min_confirmations(&other).is_err());
}
#[tokio::test]
async fn a_bitcoin_reference_without_a_depth_reads_at_zero_without_a_client() {
    let h = htlc();
    let mut history = funding(&h);
    history.transactions[0].confirmed_height = None;
    history.observation.utxos[0].confirmed_height = None;
    let reader = BitcoinReader::new(Arc::new(History(history)));
    let reference = ContractRef::Bitcoin {
        htlc: h,
        expected_sats: 10000,
        confirmations: None,
    };
    assert_eq!(
        reader.observe(&reference).await.unwrap().state,
        HtlcState::Confirmed
    );
}

#[test]
fn shared_policy_keeps_confirmation_depths_and_drops_swap_specific_fields() {
    let options = ReferenceOptions {
        client_confirmations: 3,
        server_confirmations: 6,
        #[cfg(feature = "gasless")]
        evm_from_block: 1_000,
        #[cfg(feature = "gasless")]
        evm_source_sender: Some(crate::evm::Address::repeat_byte(1)),
        #[cfg(feature = "gasless")]
        evm_lock_token: Some(crate::evm::Address::repeat_byte(2)),
    };
    let shared = options.shared_policy();
    assert_eq!(
        (shared.client_confirmations, shared.server_confirmations),
        (3, 6)
    );
    #[cfg(feature = "gasless")]
    assert_eq!(
        (
            shared.evm_from_block,
            shared.evm_source_sender,
            shared.evm_lock_token
        ),
        (0, None, None)
    );
}
