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
use crate::types::EvmHtlcKind;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolEvent;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
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
    fail_send: AtomicBool,
    settled: AtomicBool,
    funding_hash: B256,
    extra_logs: Vec<EvmLog>,
    token_balance: AtomicU64,
    domains: Mutex<Vec<Value>>,
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
            self.domains
                .lock()
                .unwrap()
                .push(data.payload["domain"]["name"].clone());
            crate::aa::signing::sign_hash(&self.secret, data.digest)
        })
    }
    fn call<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, Bytes> {
        Box::pin(async move {
            let word = if tx
                .data
                .starts_with(&crate::aa::abi::IERC20::balanceOfCall::SELECTOR)
            {
                U256::from(self.token_balance.load(Ordering::Relaxed))
            } else if tx
                .data
                .starts_with(&super::recovery::RecoveryToken::transferCall::SELECTOR)
            {
                U256::ONE
            } else if tx.data.starts_with(&Htlc::VERSIONCall::SELECTOR) {
                U256::from(self.version.load(Ordering::Relaxed))
            } else if tx.data.starts_with(&Htlc::isActiveCall::SELECTOR) {
                U256::from(u8::from(!self.settled.load(Ordering::Relaxed)))
            } else if tx.data.starts_with(&Coordinator::depositsCall::SELECTOR) {
                if self.settled.load(Ordering::Relaxed) {
                    U256::ZERO
                } else {
                    U256::from_be_slice(self.owner.as_slice())
                }
            } else {
                U256::ZERO
            };
            Ok(word.to_be_bytes::<32>().to_vec().into())
        })
    }
    fn send_transaction<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, B256> {
        Box::pin(async move {
            self.sent.lock().unwrap().push(tx.clone());
            if self.fail_send.load(Ordering::Relaxed) {
                return Err(Error::Transport("broadcast response lost".into()));
            }
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
            let mut logs = vec![log];
            if hash != self.funding_hash && self.owner != h.claimer {
                let transfer = self.sent.lock().unwrap().last().and_then(|tx| {
                    super::recovery::RecoveryToken::transferCall::abi_decode(&tx.data).ok()
                });
                logs.push(event_log(
                    super::recovery::Transfer {
                        from: if transfer.is_some() {
                            self.owner
                        } else {
                            self.coordinator
                        },
                        to: transfer.as_ref().map_or(self.owner, |t| t.to),
                        value: transfer.map_or(h.amount, |t| t.amount),
                    },
                    h.token,
                ));
            }
            logs.extend(self.extra_logs.clone());
            Ok(EvmReceipt {
                transaction_hash: hash,
                success: true,
                block_number: 10,
                logs,
            })
        })
    }
    fn block_timestamp(&self) -> WalletFuture<'_, u64> {
        Box::pin(async { Ok(self.now.load(Ordering::Relaxed)) })
    }
}

fn setup(claim: bool) -> (Client, Wallet, Arc<InMemorySwapStorage>) {
    setup_kind(claim, EvmHtlcKind::Erc20)
}
fn setup_kind(claim: bool, kind: EvmHtlcKind) -> (Client, Wallet, Arc<InMemorySwapStorage>) {
    let (token, version) = match kind {
        EvmHtlcKind::Erc20 => (Address::repeat_byte(6), 6),
        EvmHtlcKind::Native => (Address::ZERO, 1),
    };
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
        token,
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
    r["evm_htlc_version"] = json!(version);
    r["evm_htlc_kind"] = json!(kind);
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
        version: AtomicU64::new(version),
        sent: Mutex::new(vec![]),
        signatures: AtomicU64::new(0),
        fail_send: AtomicBool::new(false),
        settled: AtomicBool::new(false),
        funding_hash,
        extra_logs: Vec::new(),
        token_balance: AtomicU64::new(100_000),
        domains: Mutex::new(vec![]),
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

#[tokio::test]
async fn recovery_checks_balance_and_recipient_and_resumes_without_resending() {
    let (client, wallet, _) = setup(false);
    let mut options = DepositRecovery {
        token: wallet.htlc.token,
        destination: Address::repeat_byte(8),
        amount: wallet.htlc.amount,
        timeout: Duration::from_secs(10),
    };
    wallet.token_balance.store(1, Ordering::Relaxed);
    assert!(
        client
            .recover_evm_deposit_with_wallet("test", &wallet, options.clone())
            .await
            .is_err()
    );
    assert!(wallet.sent.lock().unwrap().is_empty());
    wallet.token_balance.store(100_000, Ordering::Relaxed);
    client
        .recover_evm_deposit_with_wallet("test", &wallet, options.clone())
        .await
        .unwrap();
    client
        .recover_evm_deposit_with_wallet("test", &wallet, options.clone())
        .await
        .unwrap();
    options.destination = Address::repeat_byte(9);
    assert!(
        client
            .recover_evm_deposit_with_wallet("test", &wallet, options)
            .await
            .is_err()
    );
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn continuation_requires_a_safe_refund_status_verified_receipt_and_available_balance() {
    let (client, wallet, store) = setup(false);
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    client
        .update_swap_metadata(
            "test",
            SwapMetadata {
                evm_refund_txid: Some(B256::repeat_byte(99).to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        client
            .get_refunded_evm_swap_continuation("test", &wallet, &options)
            .await
            .is_err()
    );
    let mut stored = store.get_swap("test").unwrap().unwrap();
    if let crate::types::GetSwapResponse::EvmToArkade(ref mut response) = stored.response {
        response.status = crate::types::SwapStatus::ClientRefundedServerFunded;
    }
    store.store_swap(stored.clone()).unwrap();
    assert!(
        client
            .get_refunded_evm_swap_continuation("test", &wallet, &options)
            .await
            .is_err()
    );
    if let crate::types::GetSwapResponse::EvmToArkade(ref mut response) = stored.response {
        response.status = crate::types::SwapStatus::ClientRefunded;
    }
    store.store_swap(stored).unwrap();
    let funds = client
        .get_refunded_evm_swap_continuation("test", &wallet, &options)
        .await
        .unwrap();
    assert_eq!(funds.amount, wallet.htlc.amount);
    wallet.token_balance.store(1, Ordering::Relaxed);
    assert!(
        client
            .get_refunded_evm_swap_continuation("test", &wallet, &options)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn funding_resume_distinguishes_pending_failed_and_successful_operations() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    let (client, _, store) = setup(false);
    let mut stored = store.get_swap("test").unwrap().unwrap();
    store.delete_swap("test").unwrap();
    stored.metadata.evm_fund_txid = None;
    stored.metadata.evm_fund_userop_hash = Some(B256::repeat_byte(8).to_string());
    store.store_swap(stored).unwrap();
    let server = MockServer::start().await;
    let bundler =
        crate::aa::bundler::BundlerClient::new(server.uri().parse().unwrap(), Default::default())
            .unwrap();
    for result in [
        Value::Null,
        json!({"success":false,"receipt":{"transactionHash":B256::repeat_byte(9)}}),
        json!({"success":true,"receipt":{"transactionHash":B256::repeat_byte(9)}}),
    ] {
        server.reset().await;
        let expected = result.clone();
        Mock::given(method("POST"))
            .respond_with(move |request: &wiremock::Request| {
                let body: Value = request.body_json().unwrap();
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
            })
            .mount(&server)
            .await;
        let resumed = client.resume_gasless_funding("test", &bundler).await;
        if expected.is_null() {
            assert_eq!(resumed.unwrap(), None);
        } else if expected["success"] == false {
            assert!(resumed.is_err());
        } else {
            assert_eq!(resumed.unwrap(), Some(B256::repeat_byte(9)));
        }
    }
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_fund_txid,
        Some(B256::repeat_byte(9).to_string())
    );
}

#[tokio::test]
async fn sponsored_claim_signs_a_kernel_batch_and_persists_before_receipt_polling() {
    use alloy::sol_types::SolValue;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    let (client, wallet, store) = setup(true);
    let server = MockServer::start().await;
    let owner = wallet.owner;
    let coordinator = wallet.coordinator;
    let persisted = store.clone();
    let operation = B256::repeat_byte(8);
    let transaction_hash = B256::repeat_byte(99);
    let sent = Arc::new(Mutex::new(false));
    let submitted = sent.clone();
    Mock::given(wiremock::matchers::method("POST")).respond_with(move |request:&wiremock::Request| {
        let body:Value=request.body_json().unwrap();
        let result=match body["method"].as_str().unwrap() {
            "eth_chainId"=>json!("0x89"),
            "eth_getTransactionCount"=>json!("0x3"),
            "eth_call"=>json!(format!("0x{:064x}",0)),
            "eth_gasPrice"=>json!("0x100000"),
            "pm_getPaymasterStubData"=>json!({"paymaster":Address::repeat_byte(20),"paymasterVerificationGasLimit":"0x10000","paymasterPostOpGasLimit":"0x10000","paymasterData":"0x","isFinal":true}),
            "eth_estimateUserOperationGas"=>json!({"callGasLimit":"0x70000","verificationGasLimit":"0x20000","preVerificationGas":"0x10000"}),
            "eth_sendUserOperation"=>{
                let op=&body["params"][0];
                assert_eq!(op["sender"].as_str().unwrap().parse::<Address>().unwrap(),owner);
                assert_eq!(op["nonce"],"0x0");
                assert_eq!(op["paymaster"].as_str().unwrap().parse::<Address>().unwrap(),Address::repeat_byte(20));
                let data:Bytes=op["callData"].as_str().unwrap().parse().unwrap();
                let execution=crate::aa::abi::IKernel::executeCall::abi_decode(&data).unwrap();
                let calls=Vec::<crate::aa::abi::Call>::abi_decode(&execution.executionCalldata).unwrap();
                assert_eq!(calls.len(),1);assert_eq!(calls[0].target,coordinator);
                assert!(calls[0].callData.starts_with(&super::claim::ClaimCoordinator::redeemAndExecuteCall::SELECTOR));
                *submitted.lock().unwrap()=true;
                json!(operation)
            },
            "eth_getUserOperationReceipt"=>{
                assert!(*submitted.lock().unwrap());
                assert_eq!(persisted.get_swap("test").unwrap().unwrap().metadata.evm_claim_userop_hash,Some(operation.to_string()));
                json!({"success":true,"receipt":{"transactionHash":transaction_hash}})
            },
            method=>panic!("unexpected RPC method {method}"),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
    }).mount(&server).await;
    let config = crate::aa::UserOpConfig {
        entry_point: Address::repeat_byte(12),
        delegation_target: Address::repeat_byte(13),
        aa: crate::aa::AaConfig {
            node_rpc_url: server.uri().parse().unwrap(),
            bundler_url: server.uri().parse().unwrap(),
            paymaster: Some(crate::aa::PaymasterConfig {
                url: server.uri().parse().unwrap(),
                context: Value::Null,
            }),
            bundler_casing: Default::default(),
            gas_overrides: None,
        },
    };
    let receipt = client
        .claim_evm_userop("test", &wallet, claim_options(&wallet), vec![], &config)
        .await
        .unwrap();
    assert_eq!(receipt.transaction_hash, Some(transaction_hash));
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_claim_txid,
        Some(transaction_hash.to_string())
    );
    assert!(
        client
            .claim_evm_with_wallet("test", &wallet, claim_options(&wallet), vec![])
            .await
            .is_err()
    );
    assert!(
        client
            .claim_evm_userop("test", &wallet, claim_options(&wallet), vec![], &config)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn native_claim_values_require_an_explicit_budget_and_reserve_submission() {
    let (client, wallet, store) = setup(true);
    let calls = vec![crate::types::DexCall {
        target: Address::repeat_byte(20).to_string(),
        data: "0x1234".into(),
        value: "123".into(),
    }];
    assert!(
        client
            .claim_evm_with_wallet("test", &wallet, claim_options(&wallet), calls.clone())
            .await
            .is_err()
    );
    assert!(
        client
            .claim_evm_with_native_fee(
                "test",
                &wallet,
                claim_options(&wallet),
                calls.clone(),
                U256::from(122)
            )
            .await
            .is_err()
    );
    assert!(wallet.sent.lock().unwrap().is_empty());
    client
        .claim_evm_with_native_fee(
            "test",
            &wallet,
            claim_options(&wallet),
            calls,
            U256::from(123),
        )
        .await
        .unwrap();
    assert_eq!(wallet.sent.lock().unwrap()[0].value, U256::from(123));
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_claim_started,
        Some(true)
    );
}

#[tokio::test]
async fn outbound_cctp_claim_reconstructs_actual_burn_and_resumes_after_expiry() {
    use crate::bridge::BridgeSubmission;
    use crate::bridge::OutboundBridge;
    use crate::bridge::OutboundClaim;
    use crate::bridge::OutboundReceipt;
    use crate::bridge::cctp;
    let (client, mut wallet, store) = setup(true);
    wallet.htlc.chain_id = 42161;
    let mut record = store.get_swap("test").unwrap().unwrap();
    let mut response = serde_json::to_value(&record.response).unwrap();
    response["evm_chain_id"] = json!(42161);
    record.response = serde_json::from_value(response).unwrap();
    store.store_swap(record).unwrap();
    let mut burn = crate::bridge::tests::burn();
    burn.source_chain_id = 42161;
    burn.source_domain = 3;
    burn.destination_domain = 6;
    burn.burn_token = cctp::CctpChain::for_chain(42161).unwrap().usdc;
    burn.sender = cctp::ARBITRUM_ADAPTER;
    let mut actual = burn.clone();
    actual.amount = U256::from(9_000_000);
    wallet.extra_logs.push(event_log(
        cctp::MessageSent {
            message: crate::bridge::tests::message(&actual).into(),
        },
        burn.source_transmitter,
    ));
    let mut options = claim_options(&wallet);
    options.sweep_token = burn.burn_token;
    options.min_amount_out = U256::ZERO;
    let request = OutboundClaim {
        dex_calls: vec![crate::types::DexCall {
            target: Address::repeat_byte(18).to_string(),
            data: "0x1234".into(),
            value: "0".into(),
        }],
        bridge: OutboundBridge::Cctp {
            burn,
            minimum_amount: U256::from(8_900_000),
        },
    };
    let result = client
        .claim_evm_to_bridge(
            "test",
            &wallet,
            options.clone(),
            request.clone(),
            BridgeSubmission::Wallet,
        )
        .await
        .unwrap();
    let OutboundReceipt::Cctp { burn: observed, .. } = result else {
        panic!("expected CCTP transfer")
    };
    assert_eq!(*observed, actual);
    wallet
        .now
        .store(wallet.htlc.timelock + 1, Ordering::Relaxed);
    let restarted = Client::builder()
        .mnemonic(MNEMONIC)
        .storage(store)
        .build()
        .unwrap();
    restarted
        .claim_evm_to_bridge(
            "test",
            &wallet,
            options.clone(),
            request.clone(),
            BridgeSubmission::Wallet,
        )
        .await
        .unwrap();
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
    let mut changed = request;
    let OutboundBridge::Cctp { burn, .. } = &mut changed.bridge else {
        unreachable!()
    };
    burn.mint_recipient = B256::repeat_byte(77);
    assert!(
        restarted
            .claim_evm_to_bridge("test", &wallet, options, changed, BridgeSubmission::Wallet)
            .await
            .is_err()
    );
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
}

fn outbound_dex_call() -> crate::types::DexCall {
    crate::types::DexCall {
        target: Address::repeat_byte(18).to_string(),
        data: "0x1234".into(),
        value: "0".into(),
    }
}
#[tokio::test]
async fn refund_lost_response_is_reserved_and_resumes_after_deposit_is_removed() {
    let (client, wallet, store) = setup(false);
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(5),
    };
    wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
    wallet.fail_send.store(true, Ordering::Relaxed);
    assert!(
        client
            .refund_evm_with_wallet("test", &wallet, options.clone())
            .await
            .is_err()
    );
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_refund_started,
        Some(true)
    );
    assert!(
        client
            .refund_evm_with_wallet("test", &wallet, options.clone())
            .await
            .is_err()
    );
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
    client
        .attach_evm_refund_transaction("test", B256::repeat_byte(99))
        .await
        .unwrap();
    wallet.settled.store(true, Ordering::Relaxed);
    client
        .refund_evm_with_wallet("test", &wallet, options.clone())
        .await
        .unwrap();
    client
        .refund_evm_with_wallet("test", &wallet, options)
        .await
        .unwrap();
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn deposit_recovery_does_not_resubmit_after_a_lost_response() {
    let (client, wallet, _) = setup(false);
    let options = DepositRecovery {
        token: wallet.htlc.token,
        destination: Address::repeat_byte(80),
        amount: U256::from(100),
        timeout: Duration::from_secs(5),
    };
    wallet.fail_send.store(true, Ordering::Relaxed);
    assert!(
        client
            .recover_evm_deposit_with_wallet("test", &wallet, options.clone())
            .await
            .is_err()
    );
    assert!(
        client
            .recover_evm_deposit_with_wallet("test", &wallet, options.clone())
            .await
            .is_err()
    );
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
    client
        .attach_evm_recovery_transaction("test", B256::repeat_byte(99))
        .await
        .unwrap();
    client
        .recover_evm_deposit_with_wallet("test", &wallet, options)
        .await
        .unwrap();
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
}

fn selector(signature: &str) -> [u8; 4] {
    let hash = alloy::primitives::keccak256(signature);
    [hash[0], hash[1], hash[2], hash[3]]
}
fn rewrite_response(store: &InMemorySwapStorage, edit: impl FnOnce(&mut Value)) {
    let mut stored = store.get_swap("test").unwrap().unwrap();
    let mut response = serde_json::to_value(&stored.response).unwrap();
    edit(&mut response);
    stored.response = serde_json::from_value(response).unwrap();
    store.store_swap(stored).unwrap();
}

#[test]
fn every_evm_leg_response_keeps_the_htlc_kind_and_defaults_to_erc20() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    for direction in [
        "lightning_to_evm",
        "bitcoin_to_evm",
        "arkade_to_evm",
        "evm_to_arkade",
        "evm_to_bitcoin",
        "evm_to_lightning",
    ] {
        let mut value = fixture[direction].clone();
        assert!(value.get("evm_htlc_kind").is_none(), "{direction}");
        let parsed: crate::types::GetSwapResponse = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&parsed).unwrap()["evm_htlc_kind"],
            "erc20",
            "{direction}"
        );
        value["evm_htlc_kind"] = json!("native");
        let parsed: crate::types::GetSwapResponse = serde_json::from_value(value).unwrap();
        assert_eq!(
            serde_json::to_value(&parsed).unwrap()["evm_htlc_kind"],
            "native",
            "{direction}"
        );
    }
}

#[tokio::test]
async fn native_refunds_send_the_four_argument_native_calls() {
    for coordinator in [true, false] {
        let (client, mut wallet, _) = setup_kind(false, EvmHtlcKind::Native);
        if !coordinator {
            wallet.htlc.sender = wallet.owner;
        }
        wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
        let options = RefundOptions {
            htlc: wallet.htlc.address,
            lock_token: Address::repeat_byte(6),
            coordinator: coordinator.then_some(wallet.coordinator),
            timeout: Duration::from_secs(10),
        };
        assert!(
            client
                .refund_evm_with_wallet("test", &wallet, options.clone())
                .await
                .is_err()
        );
        assert!(wallet.sent.lock().unwrap().is_empty());
        client
            .refund_evm_with_wallet(
                "test",
                &wallet,
                RefundOptions {
                    lock_token: Address::ZERO,
                    ..options
                },
            )
            .await
            .unwrap();
        let sent = wallet.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let (to, signature) = if coordinator {
            (
                wallet.coordinator,
                "refundTo(bytes32,uint256,address,uint256)",
            )
        } else {
            (
                wallet.htlc.address,
                "refund(bytes32,uint256,address,uint256)",
            )
        };
        assert_eq!(sent[0].to, to);
        assert_eq!(sent[0].data[..4], selector(signature));
        assert_eq!(sent[0].data.len(), 4 + 4 * 32);
    }
}

#[tokio::test]
async fn native_arkade_to_evm_claim_signs_the_native_domain_and_calls_the_native_coordinator() {
    let (client, wallet, _) = setup_kind(true, EvmHtlcKind::Native);
    client
        .claim_evm_with_wallet("test", &wallet, claim_options(&wallet), vec![])
        .await
        .unwrap();
    assert_eq!(*wallet.domains.lock().unwrap(), vec![json!("HTLCNative")]);
    let sent = wallet.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, wallet.coordinator);
    assert_eq!(
        sent[0].data[..4],
        selector(
            "redeemAndExecute(bytes32,uint256,address,uint256,(address,uint256,bytes)[],address,uint256,address,uint8,bytes32,bytes32)"
        )
    );
}

#[tokio::test]
async fn gasless_claim_refuses_a_kind_that_disagrees_with_the_lock_asset_before_signing() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::path;
    for native in [false, true] {
        let (_, wallet, store) = setup(true);
        rewrite_response(&store, |response| {
            if native {
                response["evm_htlc_kind"] = json!("native");
            } else {
                response.as_object_mut().unwrap().remove("evm_htlc_kind");
                response["wbtc_address"] = json!(Address::ZERO);
            }
        });
        let server = MockServer::start().await;
        Mock::given(path("/swap/test/redeem-and-swap-calldata"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(path("/swap/test/claim-gasless"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .build()
            .unwrap();
        let result = client
            .claim_evm_gasless("test", &wallet, claim_options(&wallet))
            .await;
        assert!(matches!(result, Err(Error::InvalidSwap(_))), "{result:?}");
        assert_eq!(wallet.signatures.load(Ordering::Relaxed), 0);
        assert!(server.received_requests().await.unwrap().is_empty());
        assert_eq!(
            store
                .get_swap("test")
                .unwrap()
                .unwrap()
                .metadata
                .evm_claim_started,
            None
        );
    }
}

#[tokio::test]
async fn native_funding_is_refused_before_any_request_or_signature() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::path;
    let (_, wallet, fixture) = setup_kind(false, EvmHtlcKind::Native);
    let response = fixture.get_swap("test").unwrap().unwrap().response;
    let store = Arc::new(InMemorySwapStorage::new());
    store
        .store_swap(StoredSwap::new(0, response, SwapMetadata::default()).unwrap())
        .unwrap();
    let server = MockServer::start().await;
    Mock::given(path("/swap/test/swap-and-lock-calldata-permit2"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store)
        .build()
        .unwrap();
    let result = client
        .fund_swap_with_wallet(
            "test",
            &wallet,
            FundingOptions {
                coordinator: wallet.coordinator,
                htlc: wallet.htlc.address,
                lock_token: Address::ZERO,
                max_source_amount: wallet.htlc.amount,
                allowed_call_targets: vec![],
                timeout: Duration::from_secs(10),
            },
        )
        .await;
    assert!(matches!(result, Err(Error::InvalidSwap(_))), "{result:?}");
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(wallet.sent.lock().unwrap().is_empty());
    assert_eq!(wallet.signatures.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn native_gasless_funding_is_refused_before_the_aa_config_or_payload() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let (_, _, fixture) = setup_kind(false, EvmHtlcKind::Native);
    let response = fixture.get_swap("test").unwrap().unwrap().response;
    let store = Arc::new(InMemorySwapStorage::new());
    store
        .store_swap(StoredSwap::new(0, response.clone(), SwapMetadata::default()).unwrap())
        .unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/swap/test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&response))
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let result = client
        .fund_swap_gasless(
            "test",
            crate::aa::GaslessOpts::new(server.uri().parse().unwrap()),
        )
        .await;
    assert!(matches!(result, Err(Error::InvalidSwap(_))), "{result:?}");
    let paths: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect();
    assert_eq!(paths, ["/swap/test"]);
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_fund_started,
        None
    );
}

#[tokio::test]
async fn native_refund_is_not_continued_through_permit2() {
    let (client, wallet, store) = setup_kind(false, EvmHtlcKind::Native);
    client
        .update_swap_metadata(
            "test",
            SwapMetadata {
                evm_refund_txid: Some(B256::repeat_byte(99).to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut stored = store.get_swap("test").unwrap().unwrap();
    if let crate::types::GetSwapResponse::EvmToArkade(ref mut response) = stored.response {
        response.status = crate::types::SwapStatus::ClientRefunded;
    }
    store.store_swap(stored).unwrap();
    let result = client
        .get_refunded_evm_swap_continuation(
            "test",
            &wallet,
            &RefundOptions {
                htlc: wallet.htlc.address,
                lock_token: Address::ZERO,
                coordinator: Some(wallet.coordinator),
                timeout: Duration::from_secs(10),
            },
        )
        .await;
    assert!(
        matches!(&result, Err(Error::InvalidSwap(message)) if message.contains("native")),
        "{result:?}"
    );
}

#[tokio::test]
async fn native_claims_are_refused_before_the_bridge_route_is_pinned() {
    use crate::bridge::BridgeSubmission;
    use crate::bridge::OutboundBridge;
    use crate::bridge::OutboundClaim;
    use crate::bridge::cctp;
    let (client, mut wallet, store) = setup_kind(true, EvmHtlcKind::Native);
    wallet.htlc.chain_id = 42161;
    rewrite_response(&store, |response| response["evm_chain_id"] = json!(42161));
    let mut burn = crate::bridge::tests::burn();
    burn.source_chain_id = 42161;
    burn.source_domain = 3;
    burn.destination_domain = 6;
    burn.burn_token = cctp::CctpChain::for_chain(42161).unwrap().usdc;
    burn.sender = cctp::ARBITRUM_ADAPTER;
    let mut options = claim_options(&wallet);
    options.sweep_token = burn.burn_token;
    options.min_amount_out = U256::ZERO;
    let request = OutboundClaim {
        dex_calls: vec![outbound_dex_call()],
        bridge: OutboundBridge::Cctp {
            burn,
            minimum_amount: U256::from(8_900_000),
        },
    };
    let result = client
        .claim_evm_to_bridge("test", &wallet, options, request, BridgeSubmission::Wallet)
        .await;
    assert!(matches!(result, Err(Error::InvalidSwap(_))), "{result:?}");
    assert_eq!(wallet.signatures.load(Ordering::Relaxed), 0);
    assert!(wallet.sent.lock().unwrap().is_empty());
    assert!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .bridge_operations
            .is_none_or(|operations| !operations.contains_key("outbound_claim"))
    );
}

#[test]
fn native_source_references_track_the_zero_asset_word() {
    use crate::observation::ContractRef;
    use crate::observation::ReferenceOptions;
    let (_, wallet, _) = setup_kind(false, EvmHtlcKind::Native);
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let mut r = fixture["evm_to_lightning"].clone();
    r["id"] = json!("test");
    r["evm_chain_id"] = json!(137);
    r["evm_htlc_address"] = json!(wallet.htlc.address);
    r["client_evm_address"] = json!(wallet.owner);
    r["server_evm_address"] = json!(wallet.htlc.claimer);
    r["hash_lock"] = json!(wallet.htlc.hash_lock);
    r["evm_refund_locktime"] = json!(wallet.htlc.timelock);
    r["evm_coordinator_address"] = json!(wallet.coordinator);
    r["evm_expected_sats"] = json!(wallet.htlc.amount.to_string());
    r["wbtc_address"] = json!(Address::ZERO);
    r["source_token"]["token_id"] = json!(Address::ZERO);
    for kind in [EvmHtlcKind::Native, EvmHtlcKind::Erc20] {
        r["evm_htlc_kind"] = json!(kind);
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
            .storage(store)
            .build()
            .unwrap();
        let references = client.contract_references("test", &ReferenceOptions::default());
        if kind == EvmHtlcKind::Erc20 {
            assert!(references.is_err());
            continue;
        }
        let Some(ContractRef::Evm { htlc, .. }) = references.unwrap().client else {
            panic!("expected an EVM source reference")
        };
        assert_eq!(htlc, wallet.htlc);
    }
}
