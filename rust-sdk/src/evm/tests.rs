use super::funding::SwapCreated;
use super::htlc::Coordinator;
use super::htlc::EvmHtlc;
use super::htlc::Htlc;
use super::htlc::SwapRedeemed;
use super::htlc::SwapRefunded;
use super::*;
use crate::Client;
use crate::InMemorySwapStorage;
use crate::Signer;
use crate::StoredSwap;
use crate::SwapMetadata;
use crate::SwapStorage;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolEvent;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
struct Wallet {
    htlc: EvmHtlc,
    coordinator: Address,
    owner: Address,
    secret: [u8; 32],
    preimage: B256,
    now: AtomicU64,
    version: AtomicU64,
    sent: Mutex<Vec<EvmTransaction>>,
    signatures: AtomicU64,
    funding_hash: B256,
}
fn event_log(event: impl SolEvent, address: Address) -> EvmLog {
    let data = event.encode_log_data();
    EvmLog {
        address,
        topics: data.topics().to_vec(),
        data: data.data,
    }
}
impl EvmWallet for Wallet {
    fn address(&self) -> Address {
        self.owner
    }
    fn chain_id(&self) -> u64 {
        self.htlc.chain_id
    }
    fn sign_typed_data<'a>(&'a self, data: &'a EvmTypedData) -> WalletFuture<'a, Signature> {
        Box::pin(async move {
            self.signatures.fetch_add(1, Ordering::Relaxed);
            crate::aa::signing::sign_hash(&self.secret, data.digest)
        })
    }
    fn call<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, Bytes> {
        Box::pin(async move {
            let word = if tx.data.starts_with(&Htlc::VERSIONCall::SELECTOR) {
                U256::from(self.version.load(Ordering::Relaxed))
            } else if tx.data.starts_with(&Htlc::isActiveCall::SELECTOR) {
                U256::ONE
            } else if tx.data.starts_with(&Coordinator::depositsCall::SELECTOR) {
                U256::from_be_slice(self.owner.as_slice())
            } else {
                U256::ZERO
            };
            Ok(word.to_be_bytes::<32>().to_vec().into())
        })
    }
    fn send_transaction<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, B256> {
        Box::pin(async move {
            self.sent.lock().unwrap().push(tx.clone());
            Ok(B256::repeat_byte(99))
        })
    }
    fn wait_for_receipt(&self, hash: B256) -> WalletFuture<'_, EvmReceipt> {
        Box::pin(async move {
            let h = &self.htlc;
            let log = if hash == self.funding_hash {
                event_log(
                    SwapCreated {
                        preimageHash: h.hash_lock,
                        refundAddress: h.sender,
                        claimAddress: h.claimer,
                        token: h.token,
                        amount: h.amount,
                        timelock: U256::from(h.timelock),
                        key: h.key(),
                    },
                    h.address,
                )
            } else if self.owner == h.claimer {
                event_log(
                    SwapRedeemed {
                        preimageHash: h.hash_lock,
                        key: h.key(),
                        preimage: self.preimage,
                    },
                    h.address,
                )
            } else {
                event_log(
                    SwapRefunded {
                        preimageHash: h.hash_lock,
                        key: h.key(),
                    },
                    h.address,
                )
            };
            Ok(EvmReceipt {
                transaction_hash: hash,
                success: true,
                block_number: 10,
                logs: vec![log],
            })
        })
    }
    fn block_timestamp(&self) -> WalletFuture<'_, u64> {
        Box::pin(async { Ok(self.now.load(Ordering::Relaxed)) })
    }
}

fn setup(claim: bool) -> (Client, Wallet, Arc<InMemorySwapStorage>) {
    let signer = Signer::from_mnemonic(MNEMONIC).unwrap();
    let key = signer.derive_evm_key(0).unwrap();
    let params = signer.derive_swap_params(0).unwrap();
    let owner = key.address.parse().unwrap();
    let coordinator = Address::repeat_byte(4);
    let server = Address::repeat_byte(3);
    let htlc = EvmHtlc {
        chain_id: 137,
        address: Address::repeat_byte(5),
        hash_lock: params.hash_lock.into(),
        amount: U256::from(100_000),
        token: Address::repeat_byte(6),
        sender: if claim { server } else { coordinator },
        claimer: if claim { owner } else { server },
        timelock: 2_000_000_000,
    };
    let mut fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let r = &mut fixture[if claim {
        "arkade_to_evm"
    } else {
        "evm_to_arkade"
    }];
    r["id"] = json!("test");
    r["evm_chain_id"] = json!(137);
    r["evm_htlc_address"] = json!(htlc.address);
    r["client_evm_address"] = json!(owner);
    r["server_evm_address"] = json!(server);
    r["hash_lock"] = json!(htlc.hash_lock);
    r["evm_refund_locktime"] = json!(htlc.timelock);
    r["evm_htlc_version"] = json!(6);
    r["evm_coordinator_address"] = json!(coordinator);
    r["wbtc_address"] = json!(htlc.token);
    r["evm_expected_sats"] = json!(htlc.amount.to_string());
    r["source_token"]["token_id"] = json!(htlc.token);
    r["target_token"]["token_id"] = json!(htlc.token);
    let funding_hash = B256::repeat_byte(42);
    let store = Arc::new(InMemorySwapStorage::new());
    store
        .store_swap(
            StoredSwap::new(
                0,
                serde_json::from_value(r.clone()).unwrap(),
                SwapMetadata {
                    evm_fund_txid: Some(funding_hash.to_string()),
                    evm_coordinator_address: Some(coordinator.to_string()),
                    ..Default::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let wallet = Wallet {
        htlc,
        coordinator,
        owner,
        secret: key.secret_key,
        preimage: params.preimage.into(),
        now: AtomicU64::new(1_999_999_000),
        version: AtomicU64::new(6),
        sent: Mutex::new(vec![]),
        signatures: AtomicU64::new(0),
        funding_hash,
    };
    (client, wallet, store)
}
fn claim_options(wallet: &Wallet) -> EvmClaimOptions {
    EvmClaimOptions {
        htlc: wallet.htlc.address,
        coordinator: wallet.coordinator,
        destination: wallet.owner,
        sweep_token: wallet.htlc.token,
        min_amount_out: wallet.htlc.amount,
        minimum_remaining_seconds: 300,
        timeout: Duration::from_secs(10),
    }
}

#[tokio::test]
async fn claim_checks_chain_funding_domain_preimage_and_deadline_before_signing() {
    let (client, wallet, store) = setup(true);
    wallet
        .now
        .store(wallet.htlc.timelock - 1, Ordering::Relaxed);
    assert!(
        client
            .claim_evm_with_wallet("test", &wallet, claim_options(&wallet), vec![])
            .await
            .is_err()
    );
    wallet
        .now
        .store(wallet.htlc.timelock - 1000, Ordering::Relaxed);
    wallet.version.store(5, Ordering::Relaxed);
    assert!(
        client
            .claim_evm_with_wallet("test", &wallet, claim_options(&wallet), vec![])
            .await
            .is_err()
    );
    assert_eq!(wallet.signatures.load(Ordering::Relaxed), 0);
    assert!(wallet.sent.lock().unwrap().is_empty());
    wallet.version.store(6, Ordering::Relaxed);
    let result = client
        .claim_evm_with_wallet("test", &wallet, claim_options(&wallet), vec![])
        .await
        .unwrap();
    assert_eq!(wallet.signatures.load(Ordering::Relaxed), 1);
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_claim_txid,
        Some(result.transaction_hash.to_string())
    );
}

#[tokio::test]
async fn refunds_use_chain_time_and_coordinator_deposit_ownership() {
    let (client, wallet, store) = setup(false);
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    assert!(
        client
            .refund_evm_with_wallet("test", &wallet, options.clone())
            .await
            .is_err()
    );
    assert!(wallet.sent.lock().unwrap().is_empty());
    wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
    let receipt = client
        .refund_evm_with_wallet("test", &wallet, options)
        .await
        .unwrap();
    let sent = wallet.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, wallet.coordinator);
    assert!(
        sent[0]
            .data
            .starts_with(&Coordinator::refundToCall::SELECTOR)
    );
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_refund_txid,
        Some(receipt.transaction_hash.to_string())
    );
}

#[tokio::test]
async fn receipts_with_wrong_contract_key_or_preimage_are_rejected() {
    let (_, wallet, _) = setup(true);
    let mut receipt = wallet
        .wait_for_receipt(B256::repeat_byte(99))
        .await
        .unwrap();
    wallet
        .htlc
        .verify_redeem_receipt(&receipt, wallet.preimage)
        .unwrap();
    assert!(
        wallet
            .htlc
            .verify_redeem_receipt(&receipt, B256::ZERO)
            .is_err()
    );
    receipt.logs[0].address = Address::ZERO;
    assert!(
        wallet
            .htlc
            .verify_redeem_receipt(&receipt, wallet.preimage)
            .is_err()
    );
}

#[tokio::test]
async fn cooperative_refund_rejects_substituted_parameters_then_signs_valid_request() {
    use alloy::primitives::keccak256;
    use alloy::sol_types::SolValue;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let (_, wallet, store) = setup(false);
    wallet.version.store(2, Ordering::Relaxed);
    let server = MockServer::start().await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let calls: Vec<crate::aa::abi::Call> = vec![];
    let params = json!({"coordinator_address":wallet.coordinator,"server_signer_address":wallet.htlc.claimer,"preimage_hash":wallet.htlc.hash_lock,"amount":wallet.htlc.amount.to_string(),"token":wallet.htlc.token,"claim_address":wallet.htlc.claimer,"timelock":wallet.htlc.timelock,"chain_id":wallet.htlc.chain_id,"sweep_token":wallet.htlc.token,"min_amount_out":"0","calls_hash":keccak256(calls.abi_encode()),"coordinator_version":2});
    let mut bad = params.clone();
    bad["chain_id"] = json!(1);
    Mock::given(path("/api/swap/test/collab-refund-evm/params"))
        .respond_with(ResponseTemplate::new(200).set_body_json(bad))
        .mount(&server)
        .await;
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    assert!(
        client
            .collab_refund_evm_with_wallet("test", &wallet, options.clone())
            .await
            .is_err()
    );
    assert_eq!(wallet.signatures.load(Ordering::Relaxed), 0);
    server.reset().await;
    Mock::given(path("/api/swap/test/collab-refund-evm/params"))
        .respond_with(ResponseTemplate::new(200).set_body_json(params))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/swap/test/collab-refund-evm"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"id":"test","tx_hash":B256::repeat_byte(9),"message":"submitted"}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    client
        .collab_refund_evm_with_wallet("test", &wallet, options)
        .await
        .unwrap();
    assert_eq!(wallet.signatures.load(Ordering::Relaxed), 1);
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_refund_txid,
        Some(B256::repeat_byte(9).to_string())
    );
}

#[tokio::test]
async fn gasless_claim_binds_the_minimum_and_persists_submission() {
    use alloy::primitives::keccak256;
    use alloy::sol_types::SolValue;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let (_, wallet, store) = setup(true);
    let server = MockServer::start().await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let calls: Vec<crate::aa::abi::Call> = vec![];
    Mock::given(path("/swap/test/redeem-and-swap-calldata")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"min_amount_out":"100000","gasless_fee_sats":0,"calls_hash":keccak256(calls.abi_encode()),"dex_calldata":null}))).mount(&server).await;
    Mock::given(method("POST"))
        .and(path("/swap/test/claim-gasless"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"id":"test","tx_hash":B256::repeat_byte(9),"message":"submitted"}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let mut options = claim_options(&wallet);
    options.min_amount_out = U256::from(100001);
    assert!(
        client
            .claim_evm_gasless("test", &wallet, options)
            .await
            .is_err()
    );
    assert_eq!(wallet.signatures.load(Ordering::Relaxed), 0);
    client
        .claim_evm_gasless("test", &wallet, claim_options(&wallet))
        .await
        .unwrap();
    assert_eq!(wallet.signatures.load(Ordering::Relaxed), 1);
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_claim_txid,
        Some(B256::repeat_byte(9).to_string())
    );
}
