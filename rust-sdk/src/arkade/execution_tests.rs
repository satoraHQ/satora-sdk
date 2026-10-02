use super::*;
use crate::InMemorySwapStorage;
use crate::Signer;
use crate::StoredSwap;
use crate::SwapStorage;
use bitcoin::key::TweakedPublicKey;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
fn pair(byte: u8) -> Keypair {
    Keypair::from_secret_key(
        &Secp256k1::new(),
        &secp256k1::SecretKey::from_slice(&[byte; 32]).unwrap(),
    )
}
struct Backend {
    info: Info,
    vtxos: Vec<VirtualTxOutPoint>,
    server: Keypair,
    submit_count: AtomicUsize,
    finalize_count: AtomicUsize,
    fail_finalize: AtomicBool,
    tamper: AtomicBool,
    final_checkpoints: Mutex<Vec<Psbt>>,
    pending: Mutex<Vec<ark_rs::core::server::PendingTx>>,
    fail_submit: AtomicBool,
    pending_queries: AtomicUsize,
    preimage_hash: ripemd160::Hash,
    intents: Mutex<Vec<Vec<OutPoint>>>,
}
impl Backend {
    /// The outpoints a GetPendingTx intent proves, checked as arkd would: each
    /// is a known VHTLC output listed once, and a leaf locked to the hash
    /// carries the preimage in its condition witness.
    fn proven_outpoints(&self, intent: &ark_rs::core::intent::Intent) -> Result<Vec<OutPoint>> {
        let condition_key = psbt::raw::Key {
            type_value: 222,
            key: VTXO_CONDITION_KEY.to_vec(),
        };
        let proof = &intent.proof;
        let mut proven = Vec::new();
        // Input 0 spends the proof's own message commitment, not a VTXO.
        for (txin, input) in proof.unsigned_tx.input.iter().zip(&proof.inputs).skip(1) {
            let outpoint = txin.previous_output;
            let Some(vtxo) = self.vtxos.iter().find(|v| v.outpoint == outpoint) else {
                return Err(invalid("intent proves an unknown VTXO"));
            };
            let prevout = TxOut {
                value: vtxo.amount,
                script_pubkey: vtxo.script.clone(),
            };
            if proven.contains(&outpoint) || input.witness_utxo.as_ref() != Some(&prevout) {
                return Err(invalid("intent input repeated or mismatched"));
            }
            let (_, (leaf, _)) = input
                .tap_scripts
                .first_key_value()
                .ok_or_else(|| invalid("intent leaf missing"))?;
            let locked = leaf
                .as_bytes()
                .windows(20)
                .any(|w| w == self.preimage_hash.as_byte_array());
            let unlocks = input
                .unknown
                .get(&condition_key)
                .filter(|w| w.len() == 34 && w[..2] == [1, 32])
                .is_some_and(|w| {
                    hash160::Hash::hash(&w[2..]).as_byte_array()
                        == self.preimage_hash.as_byte_array()
                });
            if locked && !unlocks {
                return Err(invalid("intent proof lacks the preimage"));
            }
            proven.push(outpoint);
        }
        Ok(proven)
    }
}
impl ArkadeBackend for Backend {
    fn pending(
        &self,
        intent: ark_rs::core::intent::Intent,
    ) -> ArkadeFuture<'_, Vec<ark_rs::core::server::PendingTx>> {
        Box::pin(async move {
            self.pending_queries.fetch_add(1, Ordering::Relaxed);
            let proven = self.proven_outpoints(&intent)?;
            self.intents.lock().unwrap().push(proven.clone());
            Ok(self
                .pending
                .lock()
                .unwrap()
                .iter()
                .filter(|p| {
                    p.signed_checkpoint_txs
                        .iter()
                        .all(|c| proven.contains(&c.unsigned_tx.input[0].previous_output))
                })
                .cloned()
                .collect())
        })
    }
    fn info(&self) -> ArkadeFuture<'_, Info> {
        Box::pin(async { Ok(self.info.clone()) })
    }
    fn vtxos(&self, address: ArkAddress) -> ArkadeFuture<'_, Vec<VirtualTxOutPoint>> {
        Box::pin(async move {
            let script = address.to_p2tr_script_pubkey();
            Ok(self
                .vtxos
                .iter()
                .filter(|v| v.script == script)
                .cloned()
                .collect())
        })
    }
    fn submit(
        &self,
        mut ark: Psbt,
        mut checkpoints: Vec<Psbt>,
    ) -> ArkadeFuture<'_, SubmitOffchainTxResponse> {
        Box::pin(async move {
            self.submit_count.fetch_add(1, Ordering::Relaxed);
            if self.tamper.load(Ordering::Relaxed) {
                checkpoints[0].unsigned_tx.output[0].value = Amount::from_sat(1);
            }
            sign_psbt(&mut ark, &self.server, None)?;
            for cp in &mut checkpoints {
                sign_psbt(cp, &self.server, None)?;
            }
            self.pending
                .lock()
                .unwrap()
                .push(ark_rs::core::server::PendingTx {
                    ark_txid: ark.unsigned_tx.compute_txid(),
                    signed_ark_tx: ark.clone(),
                    signed_checkpoint_txs: checkpoints.clone(),
                });
            if self.fail_submit.swap(false, Ordering::Relaxed) {
                return Err(transport("interrupted submit response"));
            }
            Ok(SubmitOffchainTxResponse {
                signed_ark_tx: ark,
                signed_checkpoint_txs: checkpoints,
            })
        })
    }
    fn finalize(&self, _: Txid, checkpoints: Vec<Psbt>) -> ArkadeFuture<'_, ()> {
        Box::pin(async move {
            self.finalize_count.fetch_add(1, Ordering::Relaxed);
            if self.fail_finalize.swap(false, Ordering::Relaxed) {
                return Err(transport("interrupted finalize"));
            }
            *self.final_checkpoints.lock().unwrap() = checkpoints;
            Ok(())
        })
    }
}
fn now() -> u32 {
    u32::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}
fn setup(direction: &str) -> (Client, Backend, Arc<InMemorySwapStorage>, String) {
    let claim = matches!(
        direction,
        "btc_to_arkade" | "evm_to_arkade" | "lightning_to_arkade"
    );
    // A fixed far-future timestamp: tests compare scripts across setups, so the
    // lock must not move with the clock.
    setup_with_lock(direction, if claim { 4_000_000_000 } else { 144 })
}
fn setup_with_lock(
    direction: &str,
    lock: u32,
) -> (Client, Backend, Arc<InMemorySwapStorage>, String) {
    let params = Signer::from_mnemonic(MNEMONIC)
        .unwrap()
        .derive_swap_params(0)
        .unwrap();
    let own = key(&hex::encode(params.public_key)).unwrap();
    let counterparty = pair(2).x_only_public_key().0;
    let server = pair(3);
    let claim = matches!(
        direction,
        "btc_to_arkade" | "evm_to_arkade" | "lightning_to_arkade"
    );
    let hash = if claim { params.hash_lock } else { [22; 32] };
    let options = StrictVhtlcOptions {
        sender: if claim { counterparty } else { own },
        receiver: if claim { own } else { counterparty },
        server: server.x_only_public_key().0,
        preimage_hash: ripemd160::Hash::hash(&hash),
        refund_locktime: lock,
        unilateral_claim_delay: Sequence::from_height(21),
        unilateral_refund_delay: Sequence::from_height(34),
        unilateral_refund_without_receiver_delay: Sequence::from_height(55),
    };
    let script = StrictVhtlcScript::new(options.clone(), Network::Regtest).unwrap();
    let mut fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let r = &mut fixture[direction];
    r["id"] = json!("arkade");
    r["network"] = json!("regtest");
    r["sender_pk"] = json!(options.sender.to_string());
    r["receiver_pk"] = json!(options.receiver.to_string());
    r["server_vhtlc_pk"] = json!(counterparty.to_string());
    r["arkade_server_pk"] = json!(options.server.to_string());
    r["hash_lock"] = json!(if direction == "btc_to_arkade" {
        options.preimage_hash.to_string()
    } else {
        hex::encode(hash)
    });
    r["vhtlc_refund_locktime"] = json!(lock);
    r["unilateral_claim_delay"] = json!(21);
    r["unilateral_refund_delay"] = json!(34);
    r["unilateral_refund_without_receiver_delay"] = json!(55);
    r["arkade_htlc_script_version"] = json!(1);
    r["target_amount"] = json!("100000");
    r["source_amount"] = json!("100000");
    if r.get("btc_vhtlc_address").is_some() {
        r["btc_vhtlc_address"] = json!(script.address().encode());
    } else {
        r["arkade_vhtlc_address"] = json!(script.address().encode());
    }
    let store = Arc::new(InMemorySwapStorage::new());
    store
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
        .storage(store.clone())
        .build()
        .unwrap();
    let info = Info {
        version: "test".into(),
        signer_pk: server.public_key(),
        forfeit_pk: server.public_key(),
        forfeit_address: Address::p2tr(&Secp256k1::new(), options.server, None, Network::Regtest),
        checkpoint_tapscript: ScriptBuf::builder()
            .push_int(1)
            .push_opcode(bitcoin::opcodes::all::OP_CSV)
            .push_opcode(bitcoin::opcodes::all::OP_DROP)
            .push_x_only_key(&options.server)
            .push_opcode(bitcoin::opcodes::all::OP_CHECKSIG)
            .into_script(),
        network: Network::Regtest,
        session_duration: 0,
        unilateral_exit_delay: Sequence::from_height(10),
        boarding_exit_delay: Sequence::from_height(10),
        utxo_min_amount: None,
        utxo_max_amount: None,
        vtxo_min_amount: Some(Amount::ONE_SAT),
        vtxo_max_amount: None,
        dust: Amount::from_sat(330),
        fees: None,
        scheduled_session: None,
        deprecated_signers: vec![],
        service_status: Default::default(),
        digest: "test".into(),
        max_tx_weight: 40_000,
        max_op_return_outputs: 3,
    };
    let vtxos = (0..2)
        .map(|vout| VirtualTxOutPoint {
            outpoint: OutPoint {
                txid: Txid::from_byte_array([4; 32]),
                vout,
            },
            created_at: 0,
            expires_at: i64::MAX,
            amount: Amount::from_sat(50_000),
            script: script.script_pubkey(),
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
        })
        .collect();
    let backend = Backend {
        info,
        vtxos,
        server,
        submit_count: AtomicUsize::new(0),
        finalize_count: AtomicUsize::new(0),
        fail_finalize: AtomicBool::new(false),
        tamper: AtomicBool::new(false),
        final_checkpoints: Mutex::new(vec![]),
        pending: Mutex::new(vec![]),
        fail_submit: AtomicBool::new(false),
        pending_queries: AtomicUsize::new(0),
        preimage_hash: options.preimage_hash,
        intents: Mutex::new(vec![]),
    };
    let destination = ArkAddress::new(
        Network::Regtest,
        options.server,
        TweakedPublicKey::dangerous_assume_tweaked(own),
    )
    .encode();
    (client, backend, store, destination)
}
fn spend_options(destination: String) -> ArkadeSpendOptions {
    ArkadeSpendOptions {
        destination,
        timeout: Duration::from_secs(5),
        minimum_remaining_seconds: 300,
    }
}

#[tokio::test]
async fn all_arkade_directions_validate_ownership_hash_and_address() {
    for direction in [
        "btc_to_arkade",
        "evm_to_arkade",
        "lightning_to_arkade",
        "arkade_to_evm",
        "arkade_to_lightning",
    ] {
        let (client, backend, _, _) = setup(direction);
        let htlc = client.arkade_htlc("arkade").unwrap();
        assert_eq!(htlc.script.script_pubkey(), backend.vtxos[0].script);
        assert_eq!(
            client
                .vhtlc_balance("arkade", &backend)
                .await
                .unwrap()
                .spendable_sats,
            100_000
        );
    }
}
#[tokio::test]
async fn multi_output_claim_resumes_finalization_without_resubmission_or_persisted_preimages() {
    let (client, backend, store, destination) = setup("btc_to_arkade");
    backend.fail_finalize.store(true, Ordering::Relaxed);
    assert!(
        client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination.clone())
            )
            .await
            .is_err()
    );
    let stored = store.get_swap("arkade").unwrap().unwrap();
    let pending = stored.metadata.arkade_pending.unwrap();
    assert!(pending.submitted && !pending.completed);
    let condition_key = psbt::raw::Key {
        type_value: 222,
        key: VTXO_CONDITION_KEY.to_vec(),
    };
    for encoded in std::iter::once(&pending.ark_tx).chain(pending.checkpoints.iter()) {
        let psbt = Psbt::from_str(encoded).unwrap();
        assert!(
            psbt.inputs.iter().all(
                |i| !i.unknown.contains_key(&condition_key) && i.final_script_witness.is_none()
            )
        );
    }
    let restarted = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store)
        .build()
        .unwrap();
    let receipt = restarted
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination.clone()),
        )
        .await
        .unwrap();
    assert_eq!(receipt.claim_amount_sats, 100_000);
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);
    assert_eq!(backend.final_checkpoints.lock().unwrap().len(), 2);
    assert!(backend.final_checkpoints.lock().unwrap().iter().all(|p| {
        p.inputs
            .iter()
            .all(|i| i.unknown.contains_key(&condition_key))
    }));
    restarted
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap();
    assert_eq!(backend.finalize_count.load(Ordering::Relaxed), 2);
}
#[tokio::test]
async fn claims_reject_underfunding_and_tampered_cosigner_transactions() {
    let (client, mut backend, _, destination) = setup("evm_to_arkade");
    backend.vtxos.pop();
    assert!(
        client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination)
            )
            .await
            .is_err()
    );
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
    let (client, backend, _, destination) = setup("lightning_to_arkade");
    backend.tamper.store(true, Ordering::Relaxed);
    assert!(
        client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination)
            )
            .await
            .is_err()
    );
    assert_eq!(backend.finalize_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn interrupted_submission_queries_pending_before_resubmitting() {
    let (client, backend, _, destination) = setup("evm_to_arkade");
    backend.fail_submit.store(true, Ordering::Relaxed);
    assert!(
        client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination.clone())
            )
            .await
            .is_err()
    );
    client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap();
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn resumed_claims_inside_the_margin_finish_only_what_may_have_been_delivered() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let risky = |destination: &str| ArkadeSpendOptions {
        minimum_remaining_seconds: 0,
        ..spend_options(destination.to_owned())
    };
    let lost_submit = || async {
        let (client, backend, store, destination) = setup_with_lock("btc_to_arkade", now() + 60);
        backend.fail_submit.store(true, Ordering::Relaxed);
        assert!(
            client
                .spend_arkade(
                    "arkade",
                    &backend,
                    None,
                    ArkadeSpend::Claim,
                    risky(&destination)
                )
                .await
                .is_err()
        );
        (client, backend, store, destination)
    };
    // An attempted submit may have reached arkd, so the claim finishes.
    let (client, backend, _, destination) = lost_submit().await;
    client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap();
    assert_eq!(backend.finalize_count.load(Ordering::Relaxed), 1);
    // With no attempt on record nothing carrying the preimage may leave, not even
    // arkd's pending query, which carries it too.
    let (client, backend, store, destination) = lost_submit().await;
    let mut stored = store.get_swap("arkade").unwrap().unwrap();
    stored.metadata.arkade_pending.as_mut().unwrap().attempted = false;
    store.store_swap(stored).unwrap();
    let error = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("insufficient time"), "{error}");
    assert_eq!(backend.pending_queries.load(Ordering::Relaxed), 0);
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);

    // A request refused at connect never reached the delegate.
    let (_, backend, store, destination) = setup_with_lock("btc_to_arkade", now() + 60);
    let unreachable = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url("http://127.0.0.1:1")
        .build()
        .unwrap();
    let delegate = |options| {
        unreachable.delegate_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            pair(5).public_key(),
            options,
        )
    };
    let sent = delegate(risky(&destination)).await.unwrap_err();
    assert!(matches!(sent, Error::Transport(_)), "{sent}");
    let state = store.get_swap("arkade").unwrap().unwrap().metadata;
    assert!(state.arkade_delegate.unwrap().unsent);
    let error = delegate(spend_options(destination.clone()))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("insufficient time"), "{error}");
    // Once a request reaches the server, the intent counts as sent again.
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(ResponseTemplate::new(500))
        .expect(2)
        .mount(&server)
        .await;
    let reachable = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .build()
        .unwrap();
    for options in [risky(&destination), spend_options(destination)] {
        let error = reachable
            .delegate_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                pair(5).public_key(),
                options,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Api { .. }), "{error}");
        let state = store.get_swap("arkade").unwrap().unwrap().metadata;
        assert!(!state.arkade_delegate.unwrap().unsent);
    }
}

#[tokio::test]
async fn claims_refuse_to_reveal_the_preimage_inside_the_refund_margin() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;

    let (client, backend, store, destination) = setup_with_lock("btc_to_arkade", now() + 60);
    let error = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination.clone()),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("insufficient time"), "{error}");
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
    let metadata = store.get_swap("arkade").unwrap().unwrap().metadata;
    assert!(metadata.arkade_pending.is_none());

    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"commitment_txid":Txid::from_byte_array([14;32])})),
        )
        .expect(0)
        .mount(&server)
        .await;
    let delegating = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .build()
        .unwrap();
    let error = delegating
        .delegate_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            pair(5).public_key(),
            spend_options(destination.clone()),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("insufficient time"), "{error}");
    assert!(server.received_requests().await.unwrap().is_empty());
    let metadata = store.get_swap("arkade").unwrap().unwrap().metadata;
    assert!(metadata.arkade_delegate.is_none());

    // A persisted delegation may already have sent its preimage, so it resubmits.
    let (_, backend, store, destination) = setup_with_lock("btc_to_arkade", now() + 60);
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(ResponseTemplate::new(500))
        .expect(2)
        .mount(&server)
        .await;
    let delegating = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .build()
        .unwrap();
    let mut accepting_risk = spend_options(destination.clone());
    accepting_risk.minimum_remaining_seconds = 0;
    assert!(
        delegating
            .delegate_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                pair(5).public_key(),
                accepting_risk,
            )
            .await
            .is_err()
    );
    assert!(
        store
            .get_swap("arkade")
            .unwrap()
            .unwrap()
            .metadata
            .arkade_delegate
            .is_some()
    );
    let error = delegating
        .delegate_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            pair(5).public_key(),
            spend_options(destination),
        )
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("insufficient time"), "{error}");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);

    let (client, backend, _, destination) = setup_with_lock("btc_to_arkade", now() + 60);
    let mut accepting_risk = spend_options(destination);
    accepting_risk.minimum_remaining_seconds = 0;
    client
        .spend_arkade("arkade", &backend, None, ArkadeSpend::Claim, accepting_risk)
        .await
        .unwrap();
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);

    let (client, backend, _, destination) = setup_with_lock("btc_to_arkade", 144);
    let error = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination.clone()),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("chain reader"), "{error}");
    // 300 seconds is one block of margin under a height lock.
    let error = client
        .spend_arkade(
            "arkade",
            &backend,
            Some(&Chain(143)),
            ArkadeSpend::Claim,
            spend_options(destination.clone()),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("insufficient time"), "{error}");
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
    client
        .spend_arkade(
            "arkade",
            &backend,
            Some(&Chain(100)),
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap();
}

struct Chain(u32);
impl crate::bitcoin::BitcoinBackend for Chain {
    fn network(&self) -> Network {
        Network::Regtest
    }
    fn observe<'a>(
        &'a self,
        _: &'a Address,
    ) -> crate::bitcoin::ChainFuture<'a, crate::bitcoin::BitcoinObservation> {
        Box::pin(async {
            Ok(crate::bitcoin::BitcoinObservation {
                tip: bitcoin::BlockHash::all_zeros(),
                height: self.0,
                median_time_past: 0,
                utxos: vec![],
            })
        })
    }
    fn transaction(&self, _: Txid) -> crate::bitcoin::ChainFuture<'_, Option<Transaction>> {
        Box::pin(async { Ok(None) })
    }
    fn fee_rate(&self, _: u16) -> crate::bitcoin::ChainFuture<'_, bitcoin::FeeRate> {
        Box::pin(async { Err(invalid("unused")) })
    }
    fn broadcast<'a>(&'a self, _: &'a Transaction) -> crate::bitcoin::ChainFuture<'a, Txid> {
        Box::pin(async { Err(invalid("unused")) })
    }
}
#[tokio::test]
async fn refund_checks_chain_maturity_and_does_not_require_the_invoice_preimage() {
    let (client, backend, _, destination) = setup("arkade_to_lightning");
    assert!(
        client
            .spend_arkade(
                "arkade",
                &backend,
                Some(&Chain(143)),
                ArkadeSpend::Refund,
                spend_options(destination.clone())
            )
            .await
            .is_err()
    );
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
    client
        .spend_arkade(
            "arkade",
            &backend,
            Some(&Chain(144)),
            ArkadeSpend::Refund,
            spend_options(destination),
        )
        .await
        .unwrap();
    let checkpoints = backend.final_checkpoints.lock().unwrap();
    assert_eq!(checkpoints[0].unsigned_tx.lock_time.to_consensus_u32(), 144);
    assert!(checkpoints.iter().all(|cp| {
        cp.inputs
            .iter()
            .all(|i| i.unknown.keys().all(|key| key.key != VTXO_CONDITION_KEY))
    }));
}
#[tokio::test]
async fn cooperative_refund_preserves_receiver_signatures_through_server_cosigning() {
    for direction in ["arkade_to_evm", "arkade_to_lightning"] {
        cooperative_refund_through_server_cosigning(direction).await;
    }
}
async fn cooperative_refund_through_server_cosigning(direction: &str) {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (_, backend, store, destination) = setup(direction);
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::path(
        "/api/swap/arkade-evm/arkade/collab-refund",
    ))
    .respond_with(|request: &wiremock::Request| {
        let body: Value = request.body_json().unwrap();
        let mut ark = restore(body["ark_tx"].as_str().unwrap(), None).unwrap();
        sign_psbt(&mut ark, &pair(2), None).unwrap();
        let cps: Vec<_> = body["checkpoint_txs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let mut p = restore(p.as_str().unwrap(), None).unwrap();
                sign_psbt(&mut p, &pair(2), None).unwrap();
                p.to_string()
            })
            .collect();
        ResponseTemplate::new(200)
            .set_body_json(json!({"ark_tx":ark.to_string(),"checkpoint_txs":cps}))
    })
    .expect(1)
    .mount(&server)
    .await;
    let mut client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store)
        .base_url(server.uri())
        .build()
        .unwrap();
    let logger = record(&mut client);
    client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::CooperativeRefund,
            spend_options(destination),
        )
        .await
        .unwrap();
    assert_eq!(
        logger.events(),
        [
            "arkade.refund.start",
            "arkade.refund.vtxos_found",
            "arkade.refund.checkpoints_signed",
            "arkade.refund.finalized",
        ]
    );
    assert!(
        logger
            .records()
            .iter()
            .all(|r| r.operation.as_deref() == Some("arkade.refund.collab"))
    );
    for cp in backend.final_checkpoints.lock().unwrap().iter() {
        assert!(
            cp.inputs[0]
                .tap_script_sigs
                .keys()
                .any(|(pk, _)| *pk == pair(2).x_only_public_key().0)
        );
        assert_eq!(cp.inputs[0].tap_script_sigs.len(), 3);
    }
}

#[tokio::test]
async fn finalized_server_psbts_are_verified_before_restoring_signature_metadata() {
    let (client, backend, _, destination) = setup("evm_to_arkade");
    client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap();
    let original = backend.final_checkpoints.lock().unwrap()[0].clone();
    let mut finalized = original.clone();
    for input in &mut finalized.inputs {
        let (control, (script, _)) = input.tap_scripts.first_key_value().unwrap();
        let mut witness = bitcoin::Witness::new();
        for signature in input.tap_script_sigs.values() {
            witness.push(signature.to_vec());
        }
        witness.push(script.as_bytes());
        witness.push(control.serialize());
        input.final_script_witness = Some(witness);
        input.tap_script_sigs.clear();
        input.tap_scripts.clear();
        input.witness_utxo = None;
    }
    let merged =
        merge_cosigned(&original, &finalized, backend.server.x_only_public_key().0).unwrap();
    assert_eq!(
        merged.inputs[0].tap_script_sigs,
        original.inputs[0].tap_script_sigs
    );
    finalized.unsigned_tx.output[0].value = Amount::from_sat(1);
    assert!(merge_cosigned(&original, &finalized, backend.server.x_only_public_key().0).is_err());
}

#[tokio::test]
async fn delegated_claim_handles_swept_outputs_and_signs_the_typescript_intent_and_forfeit_modes() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (_, mut backend, store, destination) = setup("btc_to_arkade");
    backend.vtxos[0].is_swept = true;
    backend.vtxos[0].expires_at = 0;
    let captured = Arc::new(Mutex::new(Value::Null));
    let capture = captured.clone();
    let server = MockServer::start().await;
    let commitment = Txid::from_byte_array([12; 32]);
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(move |request: &wiremock::Request| {
            *capture.lock().unwrap() = request.body_json().unwrap();
            ResponseTemplate::new(200).set_body_json(json!({"commitment_txid":commitment}))
        })
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .build()
        .unwrap();
    let cosigner = pair(5).public_key();
    assert_eq!(
        client
            .delegate_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                cosigner,
                spend_options(destination.clone())
            )
            .await
            .unwrap(),
        commitment
    );
    assert_eq!(
        client
            .delegate_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                cosigner,
                spend_options(destination)
            )
            .await
            .unwrap(),
        commitment
    );
    let body = captured.lock().unwrap();
    let proof = restore(body["intent_proof"].as_str().unwrap(), None).unwrap();
    assert_eq!(proof.inputs.len(), 3);
    assert_eq!(proof.unsigned_tx.output[0].value, Amount::from_sat(100_000));
    assert!(proof.inputs.iter().all(|i| {
        i.tap_script_sigs
            .values()
            .all(|s| s.sighash_type == TapSighashType::All)
    }));
    let forfeits = body["forfeit_psbts"].as_array().unwrap();
    assert_eq!(forfeits.len(), 1);
    let forfeit = restore(forfeits[0].as_str().unwrap(), None).unwrap();
    assert_eq!(
        forfeit.unsigned_tx.output[0].value,
        Amount::from_sat(50_330)
    );
    assert!(
        forfeit.inputs[0]
            .tap_script_sigs
            .values()
            .all(|s| s.sighash_type == TapSighashType::AllPlusAnyoneCanPay)
    );
    let params = Signer::from_mnemonic(MNEMONIC)
        .unwrap()
        .derive_swap_params(0)
        .unwrap();
    let own = key(&hex::encode(params.public_key)).unwrap();
    merge_cosigned(&proof, &proof, own).unwrap();
    merge_cosigned(&forfeit, &forfeit, own).unwrap();
    let state = store
        .get_swap("arkade")
        .unwrap()
        .unwrap()
        .metadata
        .arkade_delegate
        .unwrap();
    let stored_proof = restore(&state.intent_proof, None).unwrap();
    assert!(
        stored_proof
            .inputs
            .iter()
            .all(|i| i.unknown.keys().all(|key| key.key != VTXO_CONDITION_KEY))
    );
}

#[tokio::test]
async fn delegated_settle_outlives_the_client_request_timeout() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (_, backend, store, destination) = setup("btc_to_arkade");
    let server = MockServer::start().await;
    let commitment = Txid::from_byte_array([14; 32]);
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"commitment_txid":commitment}))
                .set_delay(Duration::from_millis(800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .http(
            reqwest::Client::builder()
                .timeout(Duration::from_millis(200))
                .build()
                .unwrap(),
        )
        .build()
        .unwrap();
    assert_eq!(
        client
            .delegate_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                pair(5).public_key(),
                spend_options(destination)
            )
            .await
            .unwrap(),
        commitment
    );
    let state = store
        .get_swap("arkade")
        .unwrap()
        .unwrap()
        .metadata
        .arkade_delegate
        .unwrap();
    assert_eq!(state.commitment_txid, Some(commitment.to_string()));
}

#[tokio::test]
async fn interrupted_delegate_settlement_reconciles_observed_commitment_without_another_request() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (_, mut backend, store, destination) = setup("arkade_to_lightning");
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .build()
        .unwrap();
    let cosigner = pair(5).public_key();
    assert!(
        client
            .delegate_arkade(
                "arkade",
                &backend,
                Some(&Chain(144)),
                ArkadeSpend::Refund,
                cosigner,
                spend_options(destination.clone())
            )
            .await
            .is_err()
    );
    let state = store
        .get_swap("arkade")
        .unwrap()
        .unwrap()
        .metadata
        .arkade_delegate
        .unwrap();
    let forfeit = restore(&state.forfeit_psbts[0], None).unwrap();
    assert_eq!(forfeit.unsigned_tx.lock_time.to_consensus_u32(), 144);
    assert!(
        forfeit.unsigned_tx.input[0]
            .sequence
            .enables_absolute_lock_time()
    );
    let commitment = Txid::from_byte_array([13; 32]);
    for v in &mut backend.vtxos {
        v.is_spent = true;
        v.settled_by = Some(commitment);
    }
    // Spent inputs alone are not proof: the batch must have paid the destination.
    // Until the indexer shows that output the error has to invite a retry.
    let error = client
        .delegate_arkade(
            "arkade",
            &backend,
            Some(&Chain(144)),
            ArkadeSpend::Refund,
            cosigner,
            spend_options(destination.clone()),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("not visible yet"), "{error}");
    let mut paid = backend.vtxos[0].clone();
    paid.outpoint = OutPoint {
        txid: commitment,
        vout: 0,
    };
    paid.amount = Amount::from_sat(state.amount_sats);
    paid.script = ArkAddress::decode(&destination)
        .unwrap()
        .to_p2tr_script_pubkey();
    paid.is_spent = false;
    paid.settled_by = None;
    paid.commitment_txids = vec![commitment];
    backend.vtxos.push(paid);
    assert_eq!(
        client
            .delegate_arkade(
                "arkade",
                &backend,
                Some(&Chain(144)),
                ArkadeSpend::Refund,
                cosigner,
                spend_options(destination)
            )
            .await
            .unwrap(),
        commitment
    );
}

/// Cosigns like the daemon's collab-refund-delegate handler, through ark-core's
/// `sign_delegate_psbts`, which signs intent inputs with SIGHASH_DEFAULT whatever
/// the PSBT asks for. A `forfeit_mode` other than ALL|ANYONECANPAY replaces the
/// forfeit signatures with ones made in that mode. Returns how many forfeits
/// were cosigned, counted only once the response is complete.
async fn mount_cosigner(
    server: &wiremock::MockServer,
    forfeit_mode: TapSighashType,
) -> Arc<AtomicUsize> {
    let cosigned = Arc::new(AtomicUsize::new(0));
    let count = cosigned.clone();
    wiremock::Mock::given(wiremock::matchers::path(
        "/api/swap/arkade-evm/arkade/collab-refund-delegate",
    ))
    .respond_with(move |request: &wiremock::Request| {
        let body: Value = request.body_json().unwrap();
        let receiver = pair(2);
        let mut proof = restore(body["intent_proof"].as_str().unwrap(), None).unwrap();
        let mut forfeits: Vec<Psbt> = body["forfeit_psbts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| restore(p.as_str().unwrap(), None).unwrap())
            .collect();
        ark_rs::core::batch::sign_delegate_psbts(
            |_, msg| {
                Ok(vec![(
                    Secp256k1::new().sign_schnorr_no_aux_rand(&msg, &receiver),
                    receiver.x_only_public_key().0,
                )])
            },
            &mut proof,
            &mut forfeits,
        )
        .unwrap();
        if forfeit_mode != TapSighashType::AllPlusAnyoneCanPay {
            for forfeit in &mut forfeits {
                let mut resigned = forfeit.clone();
                resigned.inputs[0].sighash_type = Some(forfeit_mode.into());
                sign_psbt(&mut resigned, &receiver, None).unwrap();
                forfeit.inputs[0].tap_script_sigs = resigned.inputs[0].tap_script_sigs.clone();
            }
        }
        count.store(forfeits.len(), Ordering::Relaxed);
        let forfeits: Vec<_> = forfeits.iter().map(ToString::to_string).collect();
        wiremock::ResponseTemplate::new(200)
            .set_body_json(json!({"intent_proof":proof.to_string(),"forfeit_psbts":forfeits}))
    })
    .expect(1)
    .mount(server)
    .await;
    cosigned
}

#[tokio::test]
async fn cooperative_delegate_verifies_receiver_signatures_including_forfeits() {
    for direction in ["arkade_to_evm", "arkade_to_lightning"] {
        cooperative_delegate_through_server_cosigning(direction).await;
    }
}
async fn cooperative_delegate_through_server_cosigning(direction: &str) {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (_, mut backend, store, destination) = setup(direction);
    backend.vtxos[0].is_swept = true;
    let server = MockServer::start().await;
    let cosigned = mount_cosigner(&server, TapSighashType::AllPlusAnyoneCanPay).await;
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            assert!(body["preimage"].is_null() && body["swap_id"].is_null());
            let proof = restore(body["intent_proof"].as_str().unwrap(), None).unwrap();
            merge_cosigned(&proof, &proof, pair(2).x_only_public_key().0).unwrap();
            assert!(proof.inputs.iter().any(|i| {
                i.tap_script_sigs
                    .values()
                    .any(|s| s.sighash_type == TapSighashType::Default)
            }));
            ResponseTemplate::new(200)
                .set_body_json(json!({"commitment_txid":Txid::from_byte_array([15;32])}))
        })
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store)
        .base_url(server.uri())
        .build()
        .unwrap();
    let result = client
        .settle_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::CooperativeRefund,
            Some(pair(5).public_key()),
            spend_options(destination),
        )
        .await
        .unwrap();
    assert!(matches!(
        result,
        super::super::ArkadeSettlement::Delegated { .. }
    ));
    // The unswept VTXO needs a forfeit, so the ALL|ANYONECANPAY path ran too.
    assert_eq!(cosigned.load(Ordering::Relaxed), 1);
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn cooperative_delegate_rejects_forfeit_signed_without_anyonecanpay() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (_, mut backend, store, destination) = setup("arkade_to_evm");
    backend.vtxos[0].is_swept = true;
    let server = MockServer::start().await;
    let cosigned = mount_cosigner(&server, TapSighashType::All).await;
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .build()
        .unwrap();
    // The batch appends a connector input to each forfeit, which a plain ALL
    // signature does not survive.
    let error = client
        .settle_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::CooperativeRefund,
            Some(pair(5).public_key()),
            spend_options(destination),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("cosigner signature missing"), "{error}");
    assert_eq!(cosigned.load(Ordering::Relaxed), 1);
    assert!(
        store
            .get_swap("arkade")
            .unwrap()
            .unwrap()
            .metadata
            .arkade_delegate
            .is_none()
    );
}
#[tokio::test]
async fn cooperative_refunds_of_arkade_received_swaps_never_reach_the_cosigner() {
    let (_, mut backend, store, destination) = setup("lightning_to_arkade");
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store)
        .base_url(server.uri())
        .build()
        .unwrap();
    let offchain = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::CooperativeRefund,
            spend_options(destination.clone()),
        )
        .await;
    assert!(
        matches!(&offchain, Err(Error::InvalidSwap(m)) if m.contains("direction does not match")),
        "{offchain:?}"
    );
    backend.vtxos[0].is_swept = true;
    assert!(
        client
            .vhtlc_balance("arkade", &backend)
            .await
            .unwrap()
            .recoverable_sats
            > 0
    );
    let delegated = client
        .settle_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::CooperativeRefund,
            Some(pair(5).public_key()),
            spend_options(destination),
        )
        .await;
    assert!(
        matches!(&delegated, Err(Error::InvalidSwap(m)) if m.contains("direction does not match")),
        "{delegated:?}"
    );
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
}
#[tokio::test]
async fn one_call_settle_fetches_the_cosigner_for_a_recoverable_claim() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (_, mut backend, store, destination) = setup("btc_to_arkade");
    backend.vtxos.truncate(1);
    backend.vtxos[0].amount = Amount::from_sat(100_000);
    backend.vtxos[0].is_swept = true;
    backend.vtxos[0].expires_at = 0;
    let server = MockServer::start().await;
    let cosigner = pair(5).public_key();
    let commitment = Txid::from_byte_array([16; 32]);
    Mock::given(wiremock::matchers::path("/api/delegate/cosigner-pk"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"cosigner_pk":cosigner.to_string()})),
        )
        .expect(1)
        .mount(&server)
        .await;
    // The first settle fails after persisting its intent; the retry must reuse the
    // persisted cosigner key rather than fetch it again.
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = attempts.clone();
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            assert_eq!(body["cosigner_pk"], json!(cosigner.to_string()));
            if seen.fetch_add(1, Ordering::Relaxed) == 0 {
                return ResponseTemplate::new(500);
            }
            ResponseTemplate::new(200).set_body_json(json!({"commitment_txid":commitment}))
        })
        .expect(2)
        .mount(&server)
        .await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store)
        .base_url(server.uri())
        .build()
        .unwrap();
    let error = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination.clone()),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Client::settle"), "{error}");
    assert!(
        client
            .settle_with(
                "arkade",
                &backend,
                ArkadeSpend::Claim,
                destination.clone(),
                Duration::from_secs(5),
            )
            .await
            .is_err()
    );
    let result = client
        .settle_with(
            "arkade",
            &backend,
            ArkadeSpend::Claim,
            destination,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert!(matches!(
        result,
        super::super::ArkadeSettlement::Delegated { commitment_txid } if commitment_txid == commitment
    ));
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn an_expired_delegated_intent_is_replaced_after_its_batch_grace_period() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let grace = super::super::delegate::INTENT_REPLACEMENT_GRACE_SECS;
    // Near the lock the first claim went out with margin 0 and may have delivered the
    // preimage, so its replacement must not be refused by the claim window.
    for (near_lock, new_destination) in [(false, false), (false, true), (true, false)] {
        let lock = if near_lock {
            now() + 60
        } else {
            now() + 86_400
        };
        let (_, mut backend, store, destination) = setup_with_lock("btc_to_arkade", lock);
        backend.vtxos.truncate(1);
        backend.vtxos[0].amount = Amount::from_sat(100_000);
        backend.vtxos[0].is_swept = true;
        backend.vtxos[0].expires_at = 0;
        let other = {
            let address = ArkAddress::decode(&destination).unwrap();
            ArkAddress::new(
                Network::Regtest,
                address.server(),
                TweakedPublicKey::dangerous_assume_tweaked(pair(6).x_only_public_key().0),
            )
            .encode()
        };
        let server = MockServer::start().await;
        let commitment = Txid::from_byte_array([17; 32]);
        let posts = Arc::new(AtomicUsize::new(0));
        let seen = posts.clone();
        Mock::given(wiremock::matchers::path("/api/delegate/settle"))
            .respond_with(move |_: &wiremock::Request| {
                if seen.fetch_add(1, Ordering::Relaxed) == 0 {
                    return ResponseTemplate::new(500);
                }
                ResponseTemplate::new(200).set_body_json(json!({"commitment_txid":commitment}))
            })
            .mount(&server)
            .await;
        let client = Client::builder()
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .base_url(server.uri())
            .build()
            .unwrap();
        let delegate = |destination: &str, margin: u64| {
            client.delegate_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                pair(5).public_key(),
                ArkadeSpendOptions {
                    minimum_remaining_seconds: margin,
                    ..spend_options(destination.to_owned())
                },
            )
        };
        let first_margin = if near_lock { 0 } else { 300 };
        assert!(delegate(&destination, first_margin).await.is_err());
        let requested = if new_destination {
            &other
        } else {
            &destination
        };
        age_delegation(&store, 60);
        let error = delegate(requested, 300).await.unwrap_err().to_string();
        let expected = if new_destination {
            "differs"
        } else {
            "grace period"
        };
        assert!(error.contains(expected), "{error}");
        assert_eq!(posts.load(Ordering::Relaxed), 1);
        age_delegation(&store, grace + 1);
        assert_eq!(delegate(requested, 300).await.unwrap(), commitment);
        assert_eq!(posts.load(Ordering::Relaxed), 2);
        let state = store
            .get_swap("arkade")
            .unwrap()
            .unwrap()
            .metadata
            .arkade_delegate
            .unwrap();
        assert_eq!(&state.destination, requested);
        let message: Value = serde_json::from_str(&state.intent_message).unwrap();
        assert!(message["expire_at"].as_u64().unwrap() > u64::from(now()));
    }
}

async fn delegate_claim(client: &Client, backend: &Backend, destination: &str) -> Result<Txid> {
    client
        .delegate_arkade(
            "arkade",
            backend,
            None,
            ArkadeSpend::Claim,
            pair(5).public_key(),
            spend_options(destination.to_owned()),
        )
        .await
}

fn age_delegation(store: &InMemorySwapStorage, seconds: u64) {
    let mut stored = store.get_swap("arkade").unwrap().unwrap();
    let state = stored.metadata.arkade_delegate.as_mut().unwrap();
    let mut message: Value = serde_json::from_str(&state.intent_message).unwrap();
    message["expire_at"] = json!(u64::from(now()) - seconds);
    state.intent_message = message.to_string();
    store.store_swap(stored).unwrap();
}

#[tokio::test]
async fn a_replacement_keeps_an_earlier_preimage_delivery_across_a_failed_connect() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (_, mut backend, store, destination) = setup_with_lock("btc_to_arkade", now() + 60);
    backend.vtxos.truncate(1);
    backend.vtxos[0].amount = Amount::from_sat(100_000);
    backend.vtxos[0].is_swept = true;
    backend.vtxos[0].expires_at = 0;
    let server = MockServer::start().await;
    let commitment = Txid::from_byte_array([18; 32]);
    let posts = Arc::new(AtomicUsize::new(0));
    let seen = posts.clone();
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(move |_: &wiremock::Request| {
            if seen.fetch_add(1, Ordering::Relaxed) == 0 {
                return ResponseTemplate::new(500);
            }
            ResponseTemplate::new(200).set_body_json(json!({"commitment_txid":commitment}))
        })
        .mount(&server)
        .await;
    let client = |base_url: String| {
        Client::builder()
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .base_url(base_url)
            .build()
            .unwrap()
    };
    let (reachable, unreachable) = (client(server.uri()), client("http://127.0.0.1:1".into()));
    let options = |margin| ArkadeSpendOptions {
        minimum_remaining_seconds: margin,
        ..spend_options(destination.clone())
    };
    let cosigner = pair(5).public_key();
    let claim = ArkadeSpend::Claim;
    assert!(
        reachable
            .delegate_arkade("arkade", &backend, None, claim, cosigner, options(0))
            .await
            .is_err()
    );
    age_delegation(
        &store,
        super::super::delegate::INTENT_REPLACEMENT_GRACE_SECS + 1,
    );
    let error = unreachable
        .delegate_arkade("arkade", &backend, None, claim, cosigner, options(300))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Transport(_)), "{error}");
    let state = store.get_swap("arkade").unwrap().unwrap().metadata;
    let state = state.arkade_delegate.unwrap();
    assert!(state.preimage_sent && state.unsent);
    // The replacement never left, but the first claim's preimage may have.
    let settled = reachable
        .delegate_arkade("arkade", &backend, None, claim, cosigner, options(300))
        .await
        .unwrap();
    assert_eq!(settled, commitment);
}

#[tokio::test]
async fn a_late_settlement_of_a_replaced_intent_reconciles_to_its_destination() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (_, mut backend, store, destination) = setup("btc_to_arkade");
    backend.vtxos.truncate(1);
    backend.vtxos[0].amount = Amount::from_sat(100_000);
    backend.vtxos[0].is_swept = true;
    backend.vtxos[0].expires_at = 0;
    let other = {
        let address = ArkAddress::decode(&destination).unwrap();
        ArkAddress::new(
            Network::Regtest,
            address.server(),
            TweakedPublicKey::dangerous_assume_tweaked(pair(6).x_only_public_key().0),
        )
        .encode()
    };
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(ResponseTemplate::new(500))
        .expect(2)
        .mount(&server)
        .await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .build()
        .unwrap();
    assert!(
        delegate_claim(&client, &backend, &destination)
            .await
            .is_err()
    );
    let first = store
        .get_swap("arkade")
        .unwrap()
        .unwrap()
        .metadata
        .arkade_delegate
        .unwrap();
    age_delegation(
        &store,
        super::super::delegate::INTENT_REPLACEMENT_GRACE_SECS + 1,
    );
    assert!(delegate_claim(&client, &backend, &other).await.is_err());
    // The first intent's batch settles after all and pays its own destination.
    let commitment = Txid::from_byte_array([19; 32]);
    let mut paid = backend.vtxos[0].clone();
    backend.vtxos[0].is_spent = true;
    backend.vtxos[0].settled_by = Some(commitment);
    paid.outpoint = OutPoint {
        txid: commitment,
        vout: 0,
    };
    paid.amount = Amount::from_sat(first.amount_sats);
    paid.script = ArkAddress::decode(&destination)
        .unwrap()
        .to_p2tr_script_pubkey();
    paid.is_swept = false;
    paid.commitment_txids = vec![commitment];
    backend.vtxos.push(paid);
    assert_eq!(
        delegate_claim(&client, &backend, &other).await.unwrap(),
        commitment
    );
}

#[tokio::test]
async fn one_call_settle_spends_spendable_outputs_offchain_without_the_cosigner() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (_, backend, store, destination) = setup("btc_to_arkade");
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store)
        .base_url(server.uri())
        .build()
        .unwrap();
    let result = client
        .settle_with(
            "arkade",
            &backend,
            ArkadeSpend::Claim,
            destination,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert!(matches!(
        result,
        super::super::ArkadeSettlement::Offchain(_)
    ));
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn underfunded_claim_is_refused_without_consulting_the_server() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;

    let (_, mut backend, store, destination) = setup("btc_to_arkade");
    backend.vtxos.truncate(1);
    // A server that lowered the target to what it funded must not be able to
    // move the claim floor.
    let mut lowered =
        serde_json::to_value(&store.get_swap("arkade").unwrap().unwrap().response).unwrap();
    lowered["target_amount"] = json!("50000");
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(lowered))
        .expect(0)
        .mount(&server)
        .await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store)
        .base_url(server.uri())
        .build()
        .unwrap();
    let error = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("underfunded"), "{error}");
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
}
#[tokio::test]
async fn unspent_surplus_next_to_a_spent_vtxo_keeps_the_spent_verdict() {
    use crate::observation::ArkadeReader;
    use crate::observation::ContractReader;
    use crate::observation::ContractRef;
    use crate::observation::HtlcState;

    let (client, mut backend, _, _) = setup("btc_to_arkade");
    let htlc = client.arkade_htlc("arkade").unwrap();
    let reference = ContractRef::Arkade {
        options: htlc.options,
        network: htlc.network,
        expected_sats: htlc.expected_amount.to_sat(),
    };
    backend.vtxos[0].is_spent = true;
    let reader = ArkadeReader::new(Arc::new(backend), Arc::new(Chain(100)));
    let observation = reader.observe(&reference).await.unwrap();
    assert_eq!(observation.state, HtlcState::SpentUnknown);
    assert_eq!(observation.remaining_amount, "50000");
    assert_eq!(observation.funded_amount, "100000");
}
fn push_asset_vtxos(backend: &mut Backend) {
    let asset = ark_rs::core::server::Asset {
        asset_id: ark_rs::core::asset::AssetId {
            txid: Txid::from_byte_array([8; 32]),
            group_index: 0,
        },
        amount: 1,
    };
    for vout in 0..2 {
        let mut vtxo = backend.vtxos[0].clone();
        vtxo.outpoint = OutPoint {
            txid: Txid::from_byte_array([9; 32]),
            vout,
        };
        vtxo.amount = Amount::from_sat(330);
        vtxo.assets = vec![asset.clone()];
        if vout == 1 {
            vtxo.is_spent = true;
            vtxo.spent_by = Some(Txid::from_byte_array([7; 32]));
        }
        backend.vtxos.push(vtxo);
    }
}
#[tokio::test]
async fn asset_vtxos_at_the_vhtlc_do_not_block_reads_or_spends() {
    use crate::observation::ArkadeReader;
    use crate::observation::ContractReader;
    use crate::observation::ContractRef;
    use crate::observation::HtlcState;

    let (client, mut backend, _, destination) = setup("btc_to_arkade");
    push_asset_vtxos(&mut backend);
    let balance = client.vhtlc_balance("arkade", &backend).await.unwrap();
    assert_eq!(balance.spendable_sats, 100_000);
    assert_eq!(balance.spent_sats, 0);

    let (_, mut observed, _, _) = setup("btc_to_arkade");
    push_asset_vtxos(&mut observed);
    let htlc = client.arkade_htlc("arkade").unwrap();
    let reference = ContractRef::Arkade {
        options: htlc.options,
        network: htlc.network,
        expected_sats: 100_000,
    };
    let observation = ArkadeReader::new(Arc::new(observed), Arc::new(Chain(144)))
        .observe(&reference)
        .await
        .unwrap();
    assert_eq!(observation.state, HtlcState::Confirmed);
    assert_eq!(observation.funded_amount, "100000");

    client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap();
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);
    let checkpoints = backend.final_checkpoints.lock().unwrap().clone();
    assert_eq!(checkpoints.len(), 2);
    assert!(checkpoints.iter().all(|cp| {
        cp.unsigned_tx
            .input
            .iter()
            .all(|i| i.previous_output.txid != Txid::from_byte_array([9; 32]))
    }));

    let (client, mut backend, _, destination) = setup("arkade_to_lightning");
    push_asset_vtxos(&mut backend);
    client
        .spend_arkade(
            "arkade",
            &backend,
            Some(&Chain(144)),
            ArkadeSpend::Refund,
            spend_options(destination),
        )
        .await
        .unwrap();

    let (client, mut backend, _, _) = setup("btc_to_arkade");
    push_asset_vtxos(&mut backend);
    backend.vtxos[2].outpoint = backend.vtxos[0].outpoint;
    assert!(client.vhtlc_balance("arkade", &backend).await.is_err());
    let (_, mut observed, _, _) = setup("btc_to_arkade");
    push_asset_vtxos(&mut observed);
    observed.vtxos[2].outpoint = observed.vtxos[0].outpoint;
    assert!(
        ArkadeReader::new(Arc::new(observed), Arc::new(Chain(144)))
            .observe(&reference)
            .await
            .is_err()
    );
}

fn auto_claimer(
    client: Client,
    backend: &Arc<Backend>,
    store: &InMemorySwapStorage,
    destination: &str,
) -> crate::tracker::AutoClaimer {
    let mut stored = store.get_swap("arkade").unwrap().unwrap();
    let mut response = serde_json::to_value(&stored.response).unwrap();
    response["target_arkade_address"] = json!(destination);
    stored.response = serde_json::from_value(response).unwrap();
    stored.metadata.target_address = Some(destination.into());
    store.store_swap(stored).unwrap();
    let mut claimer = crate::tracker::AutoClaimer::new(
        client,
        crate::tracker::AutoClaimConfig {
            timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .unwrap();
    claimer.arkade = Some(backend.clone());
    claimer
}

async fn auto_claim(
    claimer: &crate::tracker::AutoClaimer,
) -> Result<crate::tracker::ExecutionResult> {
    use crate::tracker::ActionExecutor;
    claimer
        .execute("arkade", &crate::actions::ActionKind::Claim)
        .await
}

#[tokio::test]
async fn auto_claim_settles_to_the_recorded_arkade_destination() {
    let (client, backend, store, destination) = setup("lightning_to_arkade");
    let backend = Arc::new(backend);
    let claimer = auto_claimer(client, &backend, &store, &destination);
    let Ok(crate::tracker::ExecutionResult::Submitted { reference }) = auto_claim(&claimer).await
    else {
        panic!("the claim is submitted");
    };
    let pending = store
        .get_swap("arkade")
        .unwrap()
        .unwrap()
        .metadata
        .arkade_pending
        .unwrap();
    assert!(pending.completed && pending.destination == destination);
    assert_eq!(
        reference,
        restore(&pending.ark_tx, None)
            .unwrap()
            .unsigned_tx
            .compute_txid()
            .to_string()
    );
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn auto_claim_retries_a_claim_refused_inside_the_margin_but_not_an_interrupted_submit() {
    let (client, backend, store, destination) = setup_with_lock("btc_to_arkade", now() + 60);
    let backend = Arc::new(backend);
    let claimer = auto_claimer(client, &backend, &store, &destination);
    let Ok(crate::tracker::ExecutionResult::NotSent(error)) = auto_claim(&claimer).await else {
        panic!("a claim inside the refund margin is refused before submission");
    };
    assert!(error.to_string().contains("insufficient time"), "{error}");
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);

    let (client, backend, store, destination) = setup("evm_to_arkade");
    let backend = Arc::new(backend);
    backend.fail_submit.store(true, Ordering::Relaxed);
    let claimer = auto_claimer(client, &backend, &store, &destination);
    assert!(auto_claim(&claimer).await.is_err());
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);
    let metadata = store.get_swap("arkade").unwrap().unwrap().metadata;
    assert!(
        metadata
            .arkade_pending
            .is_some_and(|p| p.attempted && !p.submitted)
    );
    // Resuming the journal could resubmit, so it is left to reconciliation.
    let queries = backend.pending_queries.load(Ordering::Relaxed);
    assert!(auto_claim(&claimer).await.is_err());
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);
    assert_eq!(backend.pending_queries.load(Ordering::Relaxed), queries);
}

/// Marks the inputs of every transaction arkd holds as spent, as its indexer
/// reports them once a submit reached it.
fn mark_submitted_inputs_spent(backend: &mut Backend) {
    let pending = backend.pending.lock().unwrap().clone();
    for tx in pending {
        for checkpoint in &tx.signed_checkpoint_txs {
            let spent = checkpoint.unsigned_tx.input[0].previous_output;
            for vtxo in backend.vtxos.iter_mut().filter(|v| v.outpoint == spent) {
                vtxo.is_spent = true;
                vtxo.spent_by = Some(checkpoint.unsigned_tx.compute_txid());
                vtxo.ark_txid = Some(tx.ark_txid);
            }
        }
    }
}

/// The same swap in a fresh store, as after a lost database and recovery.
/// `store_swap` merges metadata, so the journal cannot be cleared in place.
fn without_journal(store: &InMemorySwapStorage) -> (Client, Arc<InMemorySwapStorage>) {
    let stored = store.get_swap("arkade").unwrap().unwrap();
    let fresh = Arc::new(InMemorySwapStorage::new());
    fresh
        .store_swap(
            StoredSwap::new(stored.key_index, stored.response, SwapMetadata::default()).unwrap(),
        )
        .unwrap();
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(fresh.clone())
        .build()
        .unwrap();
    (client, fresh)
}

fn spent_offchain_elsewhere(backend: &mut Backend) {
    for vtxo in &mut backend.vtxos {
        vtxo.is_spent = true;
        vtxo.spent_by = Some(Txid::from_byte_array([5; 32]));
        vtxo.ark_txid = Some(Txid::from_byte_array([6; 32]));
    }
}

async fn continue_claim(client: &Client, backend: &Backend) -> Result<ClaimReceipt> {
    client
        .spend_arkade_with(
            "arkade",
            backend,
            None,
            ArkadeSpend::Claim,
            spend_options(String::new()),
            true,
        )
        .await
}

#[tokio::test]
async fn journal_free_claim_finalizes_arkds_pending_transaction() {
    let (client, mut backend, store, destination) = setup("btc_to_arkade");
    backend.fail_submit.store(true, Ordering::Relaxed);
    assert!(
        client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination.clone())
            )
            .await
            .is_err()
    );
    mark_submitted_inputs_spent(&mut backend);
    let (restored, store) = without_journal(&store);
    let receipt = restored
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        receipt.ark_txid,
        backend.pending.lock().unwrap()[0].ark_txid
    );
    assert_eq!(receipt.claim_amount_sats, 100_000);
    assert_eq!(receipt.destination, destination);
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);
    assert_eq!(backend.pending_queries.load(Ordering::Relaxed), 1);
    assert_eq!(backend.finalize_count.load(Ordering::Relaxed), 1);
    // The mock only answers an intent that proves the preimage over known
    // outputs; this pins which outputs the proof covers.
    let mut spent: Vec<_> = backend
        .vtxos
        .iter()
        .filter(|v| v.is_spent)
        .map(|v| v.outpoint)
        .collect();
    spent.sort();
    let mut intents = backend.intents.lock().unwrap().clone();
    intents.iter_mut().for_each(|proven| proven.sort());
    assert_eq!(intents, [spent]);
    let pending = store
        .get_swap("arkade")
        .unwrap()
        .unwrap()
        .metadata
        .arkade_pending
        .unwrap();
    assert!(pending.attempted && pending.submitted && pending.completed);
    let own = restored.arkade_htlc("arkade").unwrap().options.receiver;
    let condition_key = psbt::raw::Key {
        type_value: 222,
        key: VTXO_CONDITION_KEY.to_vec(),
    };
    let checkpoints = backend.final_checkpoints.lock().unwrap();
    assert_eq!(checkpoints.len(), 2);
    for checkpoint in checkpoints.iter() {
        let input = &checkpoint.inputs[0];
        assert!(input.unknown.contains_key(&condition_key));
        merge_cosigned(checkpoint, checkpoint, own).unwrap();
        merge_cosigned(checkpoint, checkpoint, backend.server.x_only_public_key().0).unwrap();
    }
}

/// Another client may order the claim's inputs differently from the indexer,
/// and arkd may return the signed checkpoints in any order.
#[tokio::test]
async fn journal_free_claim_follows_arkds_input_and_checkpoint_order() {
    let first_spent_vout =
        |checkpoints: &[Psbt]| checkpoints[0].unsigned_tx.input[0].previous_output.vout;
    for (reverse_inputs, reverse_checkpoints) in [(true, false), (false, true)] {
        let (client, mut backend, store, destination) = setup("btc_to_arkade");
        if reverse_inputs {
            backend.vtxos.reverse();
        }
        backend.fail_submit.store(true, Ordering::Relaxed);
        assert!(
            client
                .spend_arkade(
                    "arkade",
                    &backend,
                    None,
                    ArkadeSpend::Claim,
                    spend_options(destination.clone())
                )
                .await
                .is_err()
        );
        if reverse_inputs {
            backend.vtxos.reverse();
        }
        if reverse_checkpoints {
            backend.pending.lock().unwrap()[0]
                .signed_checkpoint_txs
                .reverse();
        }
        let lost = backend.pending.lock().unwrap()[0].clone();
        assert_eq!(first_spent_vout(&lost.signed_checkpoint_txs), 1);
        assert_eq!(backend.vtxos[0].outpoint.vout, 0);
        mark_submitted_inputs_spent(&mut backend);
        let (restored, store) = without_journal(&store);
        let receipt = restored
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination.clone()),
            )
            .await
            .unwrap();
        assert_eq!(receipt.ark_txid, lost.ark_txid);
        assert_eq!(receipt.claim_amount_sats, 100_000);
        assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);
        assert_eq!(backend.finalize_count.load(Ordering::Relaxed), 1);
        let pending = store
            .get_swap("arkade")
            .unwrap()
            .unwrap()
            .metadata
            .arkade_pending
            .unwrap();
        assert!(pending.completed);
        let finalized = backend.final_checkpoints.lock().unwrap();
        let inputs = &lost.signed_ark_tx.unsigned_tx.input;
        assert_eq!(finalized.len(), inputs.len());
        for (checkpoint, input) in finalized.iter().zip(inputs) {
            assert_eq!(
                input.previous_output,
                OutPoint::new(checkpoint.unsigned_tx.compute_txid(), 0)
            );
        }
    }
}

#[tokio::test]
async fn journal_free_continuation_skips_the_margin_before_the_lock_only() {
    let (client, mut backend, store, destination) = setup_with_lock("btc_to_arkade", now() + 60);
    backend.fail_submit.store(true, Ordering::Relaxed);
    let risky = ArkadeSpendOptions {
        minimum_remaining_seconds: 0,
        ..spend_options(destination.clone())
    };
    assert!(
        client
            .spend_arkade("arkade", &backend, None, ArkadeSpend::Claim, risky)
            .await
            .is_err()
    );
    mark_submitted_inputs_spent(&mut backend);
    let (restored, _) = without_journal(&store);
    restored
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap();
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);
    assert_eq!(backend.finalize_count.load(Ordering::Relaxed), 1);

    // Past the lock the offchain spend may be the server's refund.
    let (client, mut backend, _, destination) = setup_with_lock("btc_to_arkade", now() - 10);
    spent_offchain_elsewhere(&mut backend);
    let error = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("refund lock"), "{error}");
    assert_eq!(backend.pending_queries.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn journal_free_continuation_finalizes_only_claims_our_key_signed() {
    let (client, mut backend, store, destination) = setup("evm_to_arkade");
    let other = ArkAddress::new(
        Network::Regtest,
        backend.server.x_only_public_key().0,
        TweakedPublicKey::dangerous_assume_tweaked(pair(6).x_only_public_key().0),
    )
    .encode();
    backend.fail_submit.store(true, Ordering::Relaxed);
    assert!(
        client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(other.clone())
            )
            .await
            .is_err()
    );
    mark_submitted_inputs_spent(&mut backend);
    let lost = backend.pending.lock().unwrap()[0].clone();
    let own = client.arkade_htlc("arkade").unwrap().options.receiver;

    let mut stripped = lost.clone();
    for input in &mut stripped.signed_ark_tx.inputs {
        input.tap_script_sigs.retain(|(key, _), _| *key != own);
    }
    let mut forged = lost.clone();
    for input in &mut forged.signed_ark_tx.inputs {
        let (_, signature) = input
            .tap_script_sigs
            .iter_mut()
            .find(|((key, _), _)| *key == own)
            .unwrap();
        signature.signature = Secp256k1::new()
            .sign_schnorr_no_aux_rand(&secp256k1::Message::from_digest([1; 32]), &pair(9));
    }
    for candidate in [stripped, forged] {
        *backend.pending.lock().unwrap() = vec![candidate];
        let (restored, _) = without_journal(&store);
        let error = restored
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination.clone()),
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("is a standard claim signed by our receiver key"),
            "{error}"
        );
        let error = continue_claim(&restored, &backend).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("is a standard claim signed by our receiver key"),
            "{error}"
        );
    }
    assert_eq!(backend.finalize_count.load(Ordering::Relaxed), 0);

    // Our key chose that destination, so the claim finishes there and says so.
    *backend.pending.lock().unwrap() = vec![lost.clone()];
    backend.fail_finalize.store(true, Ordering::Relaxed);
    let (restored, store) = without_journal(&store);
    assert!(
        restored
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination.clone())
            )
            .await
            .is_err()
    );
    let pending = store
        .get_swap("arkade")
        .unwrap()
        .unwrap()
        .metadata
        .arkade_pending
        .unwrap();
    assert_eq!(pending.destination, other);
    assert!(pending.submitted && !pending.completed);
    let receipt = restored
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap();
    assert_eq!(receipt.destination, other);
    assert_eq!(receipt.ark_txid, lost.ark_txid);
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);
    assert_eq!(backend.finalize_count.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn untouched_vhtlc_never_queries_arkd_before_the_margin_check() {
    let (client, backend, _, destination) = setup_with_lock("btc_to_arkade", now() + 60);
    let error = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("insufficient time"), "{error}");
    assert_eq!(backend.pending_queries.load(Ordering::Relaxed), 0);
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn spent_without_pending_never_resubmits() {
    let (client, mut backend, _, destination) = setup("btc_to_arkade");
    spent_offchain_elsewhere(&mut backend);
    let error = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("holds no pending claim"),
        "{error}"
    );
    assert_eq!(backend.pending_queries.load(Ordering::Relaxed), 1);
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn refunds_of_an_offchain_spent_vhtlc_never_ask_arkd() {
    let (client, mut backend, _, destination) = setup("arkade_to_evm");
    spent_offchain_elsewhere(&mut backend);
    for (operation, chain) in [
        (ArkadeSpend::Refund, Some(Chain(144))),
        (ArkadeSpend::CooperativeRefund, None),
    ] {
        let error = client
            .spend_arkade(
                "arkade",
                &backend,
                chain
                    .as_ref()
                    .map(|c| c as &dyn crate::bitcoin::BitcoinBackend),
                operation,
                spend_options(destination.clone()),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            invalid("no spendable Arkade HTLC outputs").to_string()
        );
    }
    assert_eq!(backend.pending_queries.load(Ordering::Relaxed), 0);
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn continue_arkade_claim_never_submits() {
    let (client, mut backend, store, destination) = setup("lightning_to_arkade");
    let error = continue_claim(&client, &backend).await.unwrap_err();
    assert!(
        error.to_string().contains("no pending Arkade claim"),
        "{error}"
    );
    assert_eq!(backend.pending_queries.load(Ordering::Relaxed), 0);

    backend.fail_submit.store(true, Ordering::Relaxed);
    assert!(
        client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination.clone())
            )
            .await
            .is_err()
    );
    // An attempted journal whose entry arkd no longer holds is not resubmitted.
    let lost = std::mem::take(&mut *backend.pending.lock().unwrap());
    let error = continue_claim(&client, &backend).await.unwrap_err();
    assert!(
        error.to_string().contains("no pending Arkade claim"),
        "{error}"
    );
    assert_eq!(backend.pending_queries.load(Ordering::Relaxed), 1);

    *backend.pending.lock().unwrap() = lost;
    mark_submitted_inputs_spent(&mut backend);
    let (restored, _) = without_journal(&store);
    let receipt = continue_claim(&restored, &backend).await.unwrap();
    assert_eq!(receipt.destination, destination);
    assert_eq!(backend.finalize_count.load(Ordering::Relaxed), 1);
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 1);
}

fn record(client: &mut Client) -> Arc<crate::logging::RecordingLogger> {
    let logger = Arc::new(crate::logging::RecordingLogger::default());
    client.set_logger(logger.clone(), crate::logging::LogLevel::Trace);
    logger
}

#[tokio::test]
async fn claims_log_the_typescript_events_without_secrets() {
    let (mut client, backend, _, destination) = setup("btc_to_arkade");
    let logger = record(&mut client);
    let receipt = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        logger.events(),
        [
            "arkade.claim.start",
            "arkade.claim.vtxos_found",
            "arkade.claim.checkpoints_signed",
            "arkade.claim.finalized",
        ]
    );
    let htlc = client.arkade_htlc("arkade").unwrap();
    let start = logger.find("arkade.claim.start").unwrap();
    assert_eq!(start.level, crate::logging::LogLevel::Info);
    assert_eq!(start.module.as_deref(), Some("redeem/arkade"));
    assert_eq!(start.operation.as_deref(), Some("arkade.claim"));
    assert_eq!(start.swap_id.as_deref(), Some("arkade"));
    assert_eq!(
        Value::Object(start.data),
        json!({
            "vhtlcAddress": htlc.address().encode(),
            "network": "regtest",
            "destinationAddress": destination,
        })
    );
    let found = logger.find("arkade.claim.vtxos_found").unwrap().data;
    assert_eq!(found["totalAmount"], json!("100000"));
    assert_eq!(found["vtxoCount"], json!(2));
    let finalized = logger.find("arkade.claim.finalized").unwrap();
    assert_eq!(
        finalized.data["arkTxid"],
        json!(receipt.ark_txid.to_string())
    );
    let params = Signer::from_mnemonic(MNEMONIC)
        .unwrap()
        .derive_swap_params(0)
        .unwrap();
    let logged = format!("{:?}", logger.records());
    assert!(!logged.contains(&hex::encode(params.preimage)));
    assert!(!logged.contains(&hex::encode(params.secret)));
}

#[tokio::test]
async fn resumed_claims_log_the_continue_events() {
    let (mut client, backend, _, destination) = setup("evm_to_arkade");
    let logger = record(&mut client);
    backend.fail_finalize.store(true, Ordering::Relaxed);
    assert!(
        client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination.clone())
            )
            .await
            .is_err()
    );
    let failed = logger.find("arkade.claim.finalize_failed").unwrap();
    assert_eq!(failed.level, crate::logging::LogLevel::Error);
    assert!(
        failed
            .error
            .as_deref()
            .unwrap()
            .contains("interrupted finalize")
    );
    assert_eq!(
        logger.events().last().unwrap(),
        "arkade.claim.finalize_failed"
    );
    for (fail, last) in [
        (true, "arkade.claim.continue_finalize_failed"),
        (false, "arkade.claim.finalized"),
    ] {
        backend.fail_finalize.store(fail, Ordering::Relaxed);
        let before = logger.records().len();
        let result = client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination.clone()),
            )
            .await;
        assert_eq!(result.is_err(), fail);
        assert_eq!(
            logger.events()[before..],
            [
                "arkade.claim.start",
                "arkade.claim.continue_start",
                "arkade.claim.continue_vtxos_found",
                last,
            ]
        );
        let resumed = &logger.records()[before + 1..];
        assert!(resumed.iter().all(|record| {
            record.operation.as_deref() == Some("arkade.claim.continue")
                && !record.data.contains_key("destinationAddress")
        }));
        assert_eq!(resumed[1].data["totalAmount"], json!("100000"));
        assert_eq!(resumed[1].data["vtxoCount"], json!(2));
    }
    let failed = logger
        .find("arkade.claim.continue_finalize_failed")
        .unwrap();
    assert_eq!(failed.level, crate::logging::LogLevel::Error);
    assert!(
        failed
            .error
            .as_deref()
            .unwrap()
            .contains("interrupted finalize")
    );

    let (mut client, backend, _, destination) = setup("lightning_to_arkade");
    backend.fail_submit.store(true, Ordering::Relaxed);
    let logger = record(&mut client);
    for _ in 0..2 {
        let _ = client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination.clone()),
            )
            .await;
    }
    assert_eq!(backend.finalize_count.load(Ordering::Relaxed), 1);
    assert_eq!(
        logger.events(),
        [
            "arkade.claim.start",
            "arkade.claim.vtxos_found",
            "arkade.claim.start",
            "arkade.claim.continue_start",
            "arkade.claim.continue_vtxos_found",
            "arkade.claim.pending_txs_found",
            "arkade.claim.checkpoints_signed",
            "arkade.claim.finalized",
        ]
    );
    assert_eq!(
        logger.find("arkade.claim.pending_txs_found").unwrap().data["pendingTxCount"],
        json!(1)
    );
}

#[tokio::test]
async fn continue_arkade_claim_logs_only_the_continue_events() {
    let only_continue = |records: Vec<crate::logging::LogRecord>| {
        records.iter().all(|record| {
            record.module.as_deref() == Some("redeem/arkade")
                && record.operation.as_deref() == Some("arkade.claim.continue")
                && !record.data.contains_key("destinationAddress")
        })
    };

    let (mut client, backend, _, destination) = setup("btc_to_arkade");
    backend.fail_finalize.store(true, Ordering::Relaxed);
    assert!(
        client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination)
            )
            .await
            .is_err()
    );
    let logger = record(&mut client);
    backend.fail_finalize.store(true, Ordering::Relaxed);
    assert!(continue_claim(&client, &backend).await.is_err());
    continue_claim(&client, &backend).await.unwrap();
    assert_eq!(
        logger.events(),
        [
            "arkade.claim.continue_start",
            "arkade.claim.continue_vtxos_found",
            "arkade.claim.continue_finalize_failed",
            "arkade.claim.continue_start",
            "arkade.claim.continue_vtxos_found",
            "arkade.claim.finalized",
        ]
    );
    assert!(only_continue(logger.records()));

    let (client, mut backend, store, destination) = setup("lightning_to_arkade");
    backend.fail_submit.store(true, Ordering::Relaxed);
    assert!(
        client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination)
            )
            .await
            .is_err()
    );
    mark_submitted_inputs_spent(&mut backend);
    let (mut restored, _) = without_journal(&store);
    let logger = record(&mut restored);
    continue_claim(&restored, &backend).await.unwrap();
    assert_eq!(
        logger.events(),
        [
            "arkade.claim.continue_start",
            "arkade.claim.continue_vtxos_found",
            "arkade.claim.pending_txs_found",
            "arkade.claim.checkpoints_signed",
            "arkade.claim.finalized",
        ]
    );
    assert!(only_continue(logger.records()));
}

#[tokio::test]
async fn journal_free_claims_log_the_pending_claim_or_its_absence() {
    let (client, mut backend, store, destination) = setup("btc_to_arkade");
    backend.fail_submit.store(true, Ordering::Relaxed);
    assert!(
        client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination.clone())
            )
            .await
            .is_err()
    );
    mark_submitted_inputs_spent(&mut backend);
    let (mut restored, _) = without_journal(&store);
    let logger = record(&mut restored);
    restored
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        logger.events(),
        [
            "arkade.claim.start",
            "arkade.claim.continue_start",
            "arkade.claim.continue_vtxos_found",
            "arkade.claim.pending_txs_found",
            "arkade.claim.checkpoints_signed",
            "arkade.claim.finalized",
        ]
    );
    assert_eq!(
        logger.records()[5].operation.as_deref(),
        Some("arkade.claim.continue")
    );

    let (mut client, mut backend, _, destination) = setup("btc_to_arkade");
    spent_offchain_elsewhere(&mut backend);
    let logger = record(&mut client);
    assert!(
        client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination)
            )
            .await
            .is_err()
    );
    assert_eq!(
        logger.events(),
        [
            "arkade.claim.start",
            "arkade.claim.continue_start",
            "arkade.claim.continue_vtxos_found",
            "arkade.claim.no_pending",
            "arkade.claim.no_spendable_vtxos",
        ]
    );
    let vtxos = &logger.find("arkade.claim.no_spendable_vtxos").unwrap().data["vtxos"];
    assert_eq!(vtxos.as_array().unwrap().len(), 2);
    assert_eq!(vtxos[0]["isSpent"], json!(true));
}

#[tokio::test]
async fn refunds_log_the_typescript_refund_events() {
    let (mut client, backend, _, destination) = setup("arkade_to_lightning");
    let logger = record(&mut client);
    let receipt = client
        .spend_arkade(
            "arkade",
            &backend,
            Some(&Chain(144)),
            ArkadeSpend::Refund,
            spend_options(destination.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        logger.events(),
        [
            "arkade.refund.start",
            "arkade.refund.vtxos_found",
            "arkade.refund.checkpoints_signed",
            "arkade.refund.finalized",
        ]
    );
    let records = logger.records();
    assert!(records.iter().all(|record| {
        record.module.as_deref() == Some("refund/arkade")
            && record.operation.as_deref() == Some("arkade.refund")
            && record.data["destinationAddress"] == json!(destination)
    }));
    let script = ArkAddress::decode(&destination)
        .unwrap()
        .to_p2tr_script_pubkey()
        .to_hex_string();
    assert_eq!(
        records[1].data["destinationAddressPPkScript"],
        json!(script)
    );
    assert_eq!(records[1].data["totalAmount"], json!("100000"));
    assert_eq!(
        records[3].data["arkTxid"],
        json!(receipt.ark_txid.to_string())
    );
}

#[tokio::test]
async fn delegated_settlement_logs_its_outputs_and_a_cosigner_key_fallback() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (_, mut backend, store, destination) = setup("btc_to_arkade");
    backend.vtxos.truncate(1);
    backend.vtxos[0].amount = Amount::from_sat(100_000);
    backend.vtxos[0].is_swept = true;
    backend.vtxos[0].expires_at = 0;
    let server = MockServer::start().await;
    let commitment = Txid::from_byte_array([18; 32]);
    let posts = Arc::new(AtomicUsize::new(0));
    let seen = posts.clone();
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(move |_: &wiremock::Request| {
            if seen.fetch_add(1, Ordering::Relaxed) == 0 {
                return ResponseTemplate::new(500);
            }
            ResponseTemplate::new(200).set_body_json(json!({"commitment_txid":commitment}))
        })
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::path("/api/delegate/cosigner-pk"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let mut client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .build()
        .unwrap();
    let logger = record(&mut client);
    assert!(
        delegate_claim(&client, &backend, &destination)
            .await
            .is_err()
    );
    let found = logger.find("delegate.settle.vtxos_found").unwrap();
    assert_eq!(found.module.as_deref(), Some("delegate"));
    assert_eq!(found.operation.as_deref(), Some("delegate.settle"));
    assert_eq!(found.swap_id.as_deref(), Some("arkade"));
    assert_eq!(
        Value::Object(found.data),
        json!({
            "destinationAddress": destination,
            "networkName": "regtest",
            "vtxoCount": 1,
            "totalAmount": "100000",
        })
    );
    age_delegation(
        &store,
        super::super::delegate::INTENT_REPLACEMENT_GRACE_SECS + 1,
    );
    let settled = client
        .settle_with(
            "arkade",
            &backend,
            ArkadeSpend::Claim,
            destination,
            Duration::from_secs(120),
        )
        .await
        .unwrap();
    assert!(matches!(
        settled,
        super::super::ArkadeSettlement::Delegated { commitment_txid } if commitment_txid == commitment
    ));
    let fallback = logger.find("arkade.settle.cosignerKeyFallback").unwrap();
    assert_eq!(fallback.level, crate::logging::LogLevel::Warn);
    assert!(
        fallback.data["error"].as_str().unwrap().contains("503"),
        "{fallback:?}"
    );
}

const EVM_FUNDING: &str = "0xabababababababababababababababababababababababababababababababab";

/// An EVM to Arkade swap created to pay 100_000 sats out of a lock of 101_000
/// sats of Arbitrum tBTC, funded by this wallet's per-swap address through the
/// coordinator it recorded, whose VHTLC the daemon funded with `vhtlc` sats.
/// Also returns the swap as the server now reports it, with `server_target`.
fn converted_swap(
    vhtlc: u64,
    server_target: &str,
) -> (Backend, Arc<InMemorySwapStorage>, String, Value) {
    let (_, mut backend, store, destination) = setup("evm_to_arkade");
    let mut response =
        serde_json::to_value(&store.get_swap("arkade").unwrap().unwrap().response).unwrap();
    store.delete_swap("arkade").unwrap();
    response["evm_chain_id"] = json!(42161);
    response["evm_htlc_address"] = json!(format!("0x{}", "01".repeat(20)));
    response["evm_coordinator_address"] = json!(format!("0x{}", "02".repeat(20)));
    response["server_evm_address"] = json!(format!("0x{}", "04".repeat(20)));
    response["evm_refund_locktime"] = json!(2_000_000_000u64);
    response["target_amount"] = json!("100000");
    response["evm_expected_sats"] = json!("1010000000000000");
    response["client_evm_address"] = json!(own_evm_address());
    store
        .store_swap(
            StoredSwap::new(
                0,
                serde_json::from_value(response.clone()).unwrap(),
                SwapMetadata {
                    evm_coordinator_address: Some(format!("0x{}", "02".repeat(20))),
                    ..Default::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
    backend.vtxos[1].amount = Amount::from_sat(vhtlc - 50_000);
    response["status"] = json!("serverfunded");
    response["target_amount"] = json!(server_target);
    response["evm_fund_txid"] = json!(EVM_FUNDING);
    (backend, store, destination, response)
}

fn own_evm_address() -> String {
    Signer::from_mnemonic(MNEMONIC)
        .unwrap()
        .derive_evm_key(0)
        .unwrap()
        .address
}

async fn swap_api(refreshed: Value, refreshes: u64, settles: u64) -> wiremock::MockServer {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/swap/arkade"))
        .respond_with(ResponseTemplate::new(200).set_body_json(refreshed))
        .expect(refreshes)
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"commitment_txid": Txid::from_byte_array([15; 32])})),
        )
        .expect(settles)
        .mount(&server)
        .await;
    server
}

/// The coordinator's tBTC lock for `converted_swap`, made by the swap owner's
/// funding: what an Arbitrum node would show.
#[cfg(feature = "gasless")]
struct EvmLock {
    htlc: crate::evm::htlc::EvmHtlc,
    owner: crate::evm::Address,
}
#[cfg(feature = "gasless")]
impl EvmLock {
    fn new(locked_units: crate::evm::U256) -> Self {
        use crate::evm::Address;
        let params = Signer::from_mnemonic(MNEMONIC)
            .unwrap()
            .derive_swap_params(0)
            .unwrap();
        Self {
            htlc: crate::evm::htlc::EvmHtlc {
                chain_id: 42161,
                address: Address::repeat_byte(1),
                hash_lock: params.hash_lock.into(),
                amount: locked_units,
                token: crate::evm::lock_asset(42161).unwrap().0,
                sender: Address::repeat_byte(2),
                claimer: Address::repeat_byte(4),
                timelock: 2_000_000_000,
            },
            owner: own_evm_address().parse().unwrap(),
        }
    }
}
#[cfg(feature = "gasless")]
impl crate::evm::EvmWallet for EvmLock {
    fn address(&self) -> crate::evm::Address {
        crate::evm::Address::repeat_byte(9)
    }
    fn chain_id(&self) -> u64 {
        self.htlc.chain_id
    }
    fn sign_typed_data<'a>(
        &'a self,
        _: &'a crate::evm::EvmTypedData,
    ) -> crate::evm::WalletFuture<'a, crate::evm::Signature> {
        Box::pin(async { Err(invalid("read only")) })
    }
    fn call<'a>(
        &'a self,
        tx: &'a crate::evm::EvmTransaction,
    ) -> crate::evm::WalletFuture<'a, crate::evm::Bytes> {
        Box::pin(async move {
            use alloy::sol_types::SolCall;
            let deposits = crate::evm::htlc::Coordinator::depositsCall {
                key: self.htlc.key(),
            }
            .abi_encode();
            if tx.to != self.htlc.sender || tx.data.as_ref() != deposits.as_slice() {
                return Err(invalid("unexpected EVM call"));
            }
            Ok(crate::evm::B256::left_padding_from(self.owner.as_slice())
                .to_vec()
                .into())
        })
    }
    fn send_transaction<'a>(
        &'a self,
        _: &'a crate::evm::EvmTransaction,
    ) -> crate::evm::WalletFuture<'a, crate::evm::B256> {
        Box::pin(async { Err(invalid("read only")) })
    }
    fn wait_for_receipt(
        &self,
        hash: crate::evm::B256,
    ) -> crate::evm::WalletFuture<'_, crate::evm::EvmReceipt> {
        Box::pin(async move {
            use alloy::sol_types::SolEvent;
            if hash != EVM_FUNDING.parse::<crate::evm::B256>().unwrap() {
                return Err(Error::Timeout("receipt not served".into()));
            }
            let h = &self.htlc;
            let log = crate::evm::funding::SwapCreated {
                preimageHash: h.hash_lock,
                refundAddress: h.sender,
                claimAddress: h.claimer,
                token: h.token,
                amount: h.amount,
                timelock: crate::evm::U256::from(h.timelock),
                key: h.key(),
            }
            .encode_log_data();
            Ok(crate::evm::EvmReceipt {
                transaction_hash: hash,
                success: true,
                block_number: 1,
                logs: vec![crate::evm::EvmLog {
                    address: h.address,
                    topics: log.topics().to_vec(),
                    data: log.data,
                }],
            })
        })
    }
}

/// 100_908 sats and half a sat of tBTC: 92 sats short of the 101_000 expected,
/// so the daemon pays 99_908. The half sat is truncated, as the daemon does.
#[cfg(feature = "gasless")]
fn short_lock() -> Arc<EvmLock> {
    let units = crate::evm::U256::from(100_908u64) * crate::evm::U256::from(10u64.pow(10))
        + crate::evm::U256::from(5_000_000_000u64);
    Arc::new(EvmLock::new(units))
}

#[cfg(feature = "gasless")]
#[tokio::test]
async fn a_claim_short_by_the_evm_conversion_shortfall_succeeds() {
    let (backend, store, destination, refreshed) = converted_swap(99_908, "99908");
    let server = swap_api(refreshed, 1, 0).await;
    let mut client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .build()
        .unwrap();
    let logger = record(&mut client);
    client.set_evm_reader(short_lock());
    client.set_conversion_shortfall_limit(100);
    let receipt = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap();
    assert_eq!(receipt.claim_amount_sats, 99_908);
    let stored = store.get_swap("arkade").unwrap().unwrap();
    let GetSwapResponse::EvmToArkade(response) = &stored.response else {
        panic!("direction changed");
    };
    assert_eq!(response.target_amount, "99908");
    assert_eq!(
        stored.metadata.created_amounts.unwrap().target_amount,
        "100000"
    );
    let accepted = logger.find("arkade.claim.shortfall_accepted").unwrap();
    assert_eq!(accepted.level, crate::logging::LogLevel::Info);
    assert_eq!(accepted.swap_id.as_deref(), Some("arkade"));
    assert_eq!(accepted.data["targetAmount"], json!("100000"));
    assert_eq!(accepted.data["vhtlcAmount"], json!("99908"));
    assert_eq!(accepted.data["evmShortfallSats"], json!("92"));
}

#[cfg(feature = "gasless")]
#[tokio::test]
async fn a_claim_short_by_more_than_the_evm_conversion_shortfall_is_refused() {
    // The server's lowered target matches the VHTLC; only the chain counts.
    let (backend, store, destination, refreshed) = converted_swap(99_907, "99907");
    let server = swap_api(refreshed, 2, 0).await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .build()
        .unwrap();
    client.set_evm_reader(short_lock());
    client.set_conversion_shortfall_limit(100);
    let error = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination.clone()),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("underfunded"), "{error}");
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
    assert!(
        store
            .get_swap("arkade")
            .unwrap()
            .unwrap()
            .metadata
            .arkade_pending
            .is_none()
    );
    // Nor is it refreshed into a floor: the stored target is now the server's.
    let error = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("underfunded"), "{error}");
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
}

#[cfg(feature = "gasless")]
#[tokio::test]
async fn a_delegated_settle_short_by_the_evm_conversion_shortfall_succeeds() {
    let (mut backend, store, destination, refreshed) = converted_swap(99_908, "99908");
    backend.vtxos[0].is_swept = true;
    backend.vtxos[0].expires_at = 0;
    let server = swap_api(refreshed, 1, 1).await;
    let mut client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .build()
        .unwrap();
    let logger = record(&mut client);
    client.set_evm_reader(short_lock());
    client.set_conversion_shortfall_limit(100);
    assert_eq!(
        delegate_claim(&client, &backend, &destination)
            .await
            .unwrap(),
        Txid::from_byte_array([15; 32])
    );
    let state = store
        .get_swap("arkade")
        .unwrap()
        .unwrap()
        .metadata
        .arkade_delegate
        .unwrap();
    assert_eq!(state.amount_sats, 99_908);
    let accepted = logger.find("delegate.settle.shortfall_accepted").unwrap();
    assert_eq!(accepted.data["evmShortfallSats"], json!("92"));
}

#[cfg(feature = "gasless")]
#[tokio::test]
async fn a_delegated_settle_short_by_more_than_the_evm_conversion_shortfall_is_refused() {
    let (mut backend, store, destination, refreshed) = converted_swap(99_907, "99907");
    backend.vtxos[0].is_swept = true;
    backend.vtxos[0].expires_at = 0;
    let server = swap_api(refreshed, 1, 0).await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .build()
        .unwrap();
    client.set_evm_reader(short_lock());
    client.set_conversion_shortfall_limit(100);
    let error = delegate_claim(&client, &backend, &destination)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("underfunded"), "{error}");
    assert!(
        store
            .get_swap("arkade")
            .unwrap()
            .unwrap()
            .metadata
            .arkade_delegate
            .is_none()
    );
}

#[tokio::test]
async fn a_claim_below_the_created_target_needs_an_evm_reader() {
    for delegated in [false, true] {
        let (mut backend, store, destination, refreshed) = converted_swap(99_908, "99908");
        if delegated {
            backend.vtxos[0].is_swept = true;
            backend.vtxos[0].expires_at = 0;
        }
        let server = swap_api(refreshed, 0, 0).await;
        let client = Client::builder()
            .mnemonic(MNEMONIC)
            .storage(store)
            .base_url(server.uri())
            .build()
            .unwrap();
        #[cfg(feature = "gasless")]
        client.set_conversion_shortfall_limit(100);
        let error = if delegated {
            delegate_claim(&client, &backend, &destination)
                .await
                .unwrap_err()
        } else {
            client
                .spend_arkade(
                    "arkade",
                    &backend,
                    None,
                    ArkadeSpend::Claim,
                    spend_options(destination),
                )
                .await
                .unwrap_err()
        };
        assert!(
            error.to_string().contains("underfunded") && error.to_string().contains("EVM reader"),
            "{error}"
        );
        assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
    }
}

#[cfg(feature = "gasless")]
#[tokio::test]
async fn a_shortfall_counts_only_from_the_owners_own_lock_of_the_chains_btc_token() {
    for (case, refusal) in [
        ("token", "does not contain this HTLC"),
        ("depositor", "depositor does not match"),
    ] {
        let (backend, store, destination, refreshed) = converted_swap(99_908, "99908");
        let server = swap_api(refreshed, 1, 0).await;
        let client = Client::builder()
            .mnemonic(MNEMONIC)
            .storage(store)
            .base_url(server.uri())
            .build()
            .unwrap();
        let mut lock = EvmLock::new(short_lock().htlc.amount);
        if case == "token" {
            lock.htlc.token = crate::evm::Address::repeat_byte(7);
        } else {
            lock.owner = crate::evm::Address::repeat_byte(8);
        }
        client.set_evm_reader(Arc::new(lock));
        client.set_conversion_shortfall_limit(100);
        let error = client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains(refusal), "{case}: {error}");
        assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
    }
}

#[cfg(feature = "gasless")]
#[tokio::test]
async fn a_refresh_naming_another_evm_lock_is_refused_and_not_stored() {
    let (backend, store, destination, mut refreshed) = converted_swap(99_908, "99908");
    refreshed["evm_htlc_address"] = json!(format!("0x{}", "0e".repeat(20)));
    let server = swap_api(refreshed, 1, 0).await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .base_url(server.uri())
        .build()
        .unwrap();
    client.set_evm_reader(short_lock());
    client.set_conversion_shortfall_limit(100);
    let error = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("differ"), "{error}");
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
    let GetSwapResponse::EvmToArkade(stored) = store.get_swap("arkade").unwrap().unwrap().response
    else {
        panic!("direction changed");
    };
    assert_eq!(stored.target_amount, "100000");
    assert_eq!(stored.evm_htlc_address, format!("0x{}", "01".repeat(20)));
}

#[cfg(feature = "gasless")]
#[tokio::test]
async fn a_shortfall_beyond_the_clients_limit_is_refused_before_anything_is_read() {
    // 92 sats short of a 100_000-sat target is 9.2 basis points.
    let (backend, store, destination, refreshed) = converted_swap(99_908, "99908");
    let server = swap_api(refreshed, 1, 0).await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store)
        .base_url(server.uri())
        .build()
        .unwrap();
    client.set_evm_reader(short_lock());
    for limit in [0, 9] {
        client.set_conversion_shortfall_limit(limit);
        let error = client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination.clone()),
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains(&format!(
                "Arkade HTLC is underfunded: the VHTLC holds 99908 sats, 92 below the target of 100000, which is more than this client's limit of {limit} basis points; raise it with Client::set_conversion_shortfall_limit"
            )),
            "{limit}: {error}"
        );
    }
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
    client.set_conversion_shortfall_limit(10);
    let receipt = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap();
    assert_eq!(receipt.claim_amount_sats, 99_908);
}

#[cfg(feature = "gasless")]
#[tokio::test]
async fn an_expected_lock_inflated_at_creation_cannot_lower_a_claim_past_the_limit() {
    // The chain lock of the honest case, but the creation response named an
    // expected lock of 140_908 sats, which by the daemon's formula alone would
    // make a 60_000-sat VHTLC enough.
    let (backend, store, destination, mut refreshed) = converted_swap(60_000, "60000");
    let created = store.get_swap("arkade").unwrap().unwrap();
    let mut response = serde_json::to_value(&created.response).unwrap();
    response["evm_expected_sats"] = json!("1409080000000000");
    refreshed["evm_expected_sats"] = json!("1409080000000000");
    store.delete_swap("arkade").unwrap();
    store
        .store_swap(
            StoredSwap::new(
                0,
                serde_json::from_value(response).unwrap(),
                SwapMetadata {
                    created_amounts: None,
                    ..created.metadata
                },
            )
            .unwrap(),
        )
        .unwrap();
    let server = swap_api(refreshed, 0, 0).await;
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store)
        .base_url(server.uri())
        .build()
        .unwrap();
    client.set_evm_reader(short_lock());
    client.set_conversion_shortfall_limit(100);
    let error = client
        .spend_arkade(
            "arkade",
            &backend,
            None,
            ArkadeSpend::Claim,
            spend_options(destination),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("basis points"), "{error}");
    assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
}

#[cfg(feature = "gasless")]
#[tokio::test]
async fn a_shortfall_counts_only_from_a_lock_this_wallet_funded() {
    use crate::evm::U256;
    for (case, refusal) in [
        ("owner", "owner is not an address of this wallet"),
        (
            "coordinator",
            "coordinator this wallet funded the swap through is unknown",
        ),
    ] {
        let (backend, store, destination, mut refreshed) = converted_swap(60_000, "60000");
        // A 61_000-sat lock under the swap's hash through the same coordinator,
        // which would make a 60_000-sat VHTLC enough.
        let mut lock = EvmLock::new(U256::from(61_000u64) * U256::from(10u64.pow(10)));
        let server_key = crate::evm::Address::repeat_byte(0x55);
        let refreshes = if case == "owner" {
            // An earlier refresh named the server's own key as the swap's owner.
            refreshed["client_evm_address"] = json!(server_key.to_string());
            lock.owner = server_key;
            2
        } else {
            // No funding of this wallet's recorded the coordinator.
            let mut stored = store.get_swap("arkade").unwrap().unwrap();
            stored.metadata.evm_coordinator_address = None;
            store.delete_swap("arkade").unwrap();
            store.store_swap(stored).unwrap();
            1
        };
        let server = swap_api(refreshed, refreshes, 0).await;
        let client = Client::builder()
            .mnemonic(MNEMONIC)
            .storage(store)
            .base_url(server.uri())
            .build()
            .unwrap();
        client.set_evm_reader(Arc::new(lock));
        client.set_conversion_shortfall_limit(10_000);
        if case == "owner" {
            client.refresh_swap("arkade").await.unwrap();
        }
        let error = client
            .spend_arkade(
                "arkade",
                &backend,
                None,
                ArkadeSpend::Claim,
                spend_options(destination),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains(refusal), "{case}: {error}");
        assert_eq!(backend.submit_count.load(Ordering::Relaxed), 0);
    }
}

#[cfg(feature = "gasless")]
#[tokio::test]
async fn a_replaced_delegated_claim_that_may_have_sent_the_preimage_skips_the_amount_check() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (mut backend, store, destination, refreshed) = converted_swap(99_908, "99908");
    backend.vtxos[0].is_swept = true;
    backend.vtxos[0].expires_at = 0;
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/swap/arkade"))
        .respond_with(ResponseTemplate::new(200).set_body_json(refreshed))
        .expect(1)
        .mount(&server)
        .await;
    let commitment = Txid::from_byte_array([20; 32]);
    let posts = Arc::new(AtomicUsize::new(0));
    let seen = posts.clone();
    Mock::given(wiremock::matchers::path("/api/delegate/settle"))
        .respond_with(move |_: &wiremock::Request| {
            if seen.fetch_add(1, Ordering::Relaxed) == 0 {
                return ResponseTemplate::new(500);
            }
            ResponseTemplate::new(200).set_body_json(json!({"commitment_txid":commitment}))
        })
        .mount(&server)
        .await;
    let client = || {
        Client::builder()
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .base_url(server.uri())
            .build()
            .unwrap()
    };
    let checked = client();
    checked.set_evm_reader(short_lock());
    checked.set_conversion_shortfall_limit(100);
    assert!(
        delegate_claim(&checked, &backend, &destination)
            .await
            .is_err()
    );
    age_delegation(
        &store,
        super::super::delegate::INTENT_REPLACEMENT_GRACE_SECS + 1,
    );
    // Neither a reader nor a limit: the server may already hold the preimage, so
    // refusing the replacement would only strand the claim.
    assert_eq!(
        delegate_claim(&client(), &backend, &destination)
            .await
            .unwrap(),
        commitment
    );
    assert_eq!(posts.load(Ordering::Relaxed), 2);
}

#[cfg(feature = "gasless")]
#[tokio::test]
async fn the_lock_read_refuses_a_hash_lock_this_wallet_did_not_derive() {
    // What a refresh racing the claim could leave behind after the claim checked
    // its VHTLC against the stored hash lock.
    let (_, store, _, _) = converted_swap(60_000, "60000");
    let mut stored = store.get_swap("arkade").unwrap().unwrap();
    let mut response = serde_json::to_value(&stored.response).unwrap();
    response["hash_lock"] = json!(format!("0x{}", "22".repeat(32)));
    stored.response = serde_json::from_value(response).unwrap();
    store.delete_swap("arkade").unwrap();
    store.store_swap(stored).unwrap();
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store)
        .build()
        .unwrap();
    let mut lock = EvmLock::new(crate::evm::U256::from(61_000u64));
    lock.htlc.hash_lock = crate::evm::B256::repeat_byte(0x22);
    let error = client
        .owner_funded_evm_htlc("arkade", &lock, lock.htlc.token)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("hash lock is not this wallet's"),
        "{error}"
    );
}
