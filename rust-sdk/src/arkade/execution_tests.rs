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
}
impl ArkadeBackend for Backend {
    fn pending(
        &self,
        _: ark_rs::core::intent::Intent,
    ) -> ArkadeFuture<'_, Vec<ark_rs::core::server::PendingTx>> {
        Box::pin(async { Ok(self.pending.lock().unwrap().clone()) })
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
fn setup(direction: &str) -> (Client, Backend, Arc<InMemorySwapStorage>, String) {
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
        refund_locktime: 144,
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
    r["vhtlc_refund_locktime"] = json!(144);
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
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (_, backend, store, destination) = setup("arkade_to_evm");
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
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store)
        .base_url(server.uri())
        .build()
        .unwrap();
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
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (_, mut backend, store, destination) = setup("arkade_to_evm");
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
