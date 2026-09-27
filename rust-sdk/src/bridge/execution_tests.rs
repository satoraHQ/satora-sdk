use super::execution::*;
use super::*;
use crate::Client;
use crate::Error;
use crate::InMemorySwapStorage;
use crate::StoredSwap;
use crate::SwapMetadata;
use crate::SwapStorage;
use crate::control::PollOptions;
use crate::evm::*;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolEvent;
use alloy::sol_types::SolValue;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

struct Wallet {
    chain_id: u64,
    address: Address,
    calls: Mutex<Vec<EvmTransaction>>,
    receipt: EvmReceipt,
    fail_receipt: AtomicBool,
    lose_submission: AtomicBool,
    allowance: U256,
    used_nonce: bool,
    now: AtomicU64,
}
impl EvmWallet for Wallet {
    fn address(&self) -> Address {
        self.address
    }
    fn chain_id(&self) -> u64 {
        self.chain_id
    }
    fn sign_typed_data<'a>(&'a self, _: &'a EvmTypedData) -> WalletFuture<'a, Signature> {
        Box::pin(async { panic!("bridge wallet needs no typed signature") })
    }
    fn call<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, Bytes> {
        Box::pin(async move {
            let word = if tx
                .data
                .starts_with(&crate::aa::abi::IERC20::balanceOfCall::SELECTOR)
            {
                U256::from(100_000_000)
            } else if tx
                .data
                .starts_with(&execution::Token::allowanceCall::SELECTOR)
            {
                self.allowance
            } else if tx.data.starts_with(&cctp::usedNoncesCall::SELECTOR) {
                U256::from(u64::from(self.used_nonce))
            } else if tx.data.starts_with(&usdt0::quoteSendCall::SELECTOR) {
                return Ok(usdt0::MessagingFee {
                    nativeFee: U256::from(10),
                    lzTokenFee: U256::ZERO,
                }
                .abi_encode()
                .into());
            } else {
                U256::ONE
            };
            Ok(word.to_be_bytes::<32>().to_vec().into())
        })
    }
    fn send_transaction<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, B256> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(tx.clone());
            if self.lose_submission.load(Ordering::Relaxed) {
                return Err(Error::Transport("response lost".into()));
            }
            Ok(self.receipt.transaction_hash)
        })
    }
    fn wait_for_receipt(&self, _: B256) -> WalletFuture<'_, EvmReceipt> {
        Box::pin(async move {
            if self.fail_receipt.load(Ordering::Relaxed) {
                return Err(Error::Transport("offline".into()));
            }
            Ok(self.receipt.clone())
        })
    }
    fn block_timestamp(&self) -> WalletFuture<'_, u64> {
        Box::pin(async move { Ok(self.now.load(Ordering::Relaxed)) })
    }
}
fn log(address: Address, event: impl SolEvent) -> EvmLog {
    let data = event.encode_log_data();
    EvmLog {
        address,
        topics: data.topics().to_vec(),
        data: data.data,
    }
}
fn wallet(chain_id: u64, address: Address, logs: Vec<EvmLog>) -> Wallet {
    Wallet {
        chain_id,
        address,
        calls: Mutex::new(Vec::new()),
        receipt: EvmReceipt {
            transaction_hash: B256::repeat_byte(8),
            block_number: 2,
            success: true,
            logs,
        },
        fail_receipt: AtomicBool::new(false),
        lose_submission: AtomicBool::new(false),
        allowance: U256::MAX,
        used_nonce: false,
        now: AtomicU64::new(1_900_000_000),
    }
}
fn client() -> (Client, String, Arc<InMemorySwapStorage>) {
    let storage = Arc::new(InMemorySwapStorage::new());
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let response: crate::types::GetSwapResponse =
        serde_json::from_value(fixtures["evm_to_arkade"].clone()).unwrap();
    let id = response.id().to_string();
    storage
        .store_swap(StoredSwap::new(0, response, SwapMetadata::default()).unwrap())
        .unwrap();
    (
        Client::builder()
            .base_url("http://localhost")
            .mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about")
            .storage(storage.clone())
            .build()
            .unwrap(),
        id,
        storage,
    )
}
#[tokio::test]
async fn interrupted_burn_reuses_receipt_and_rejects_changed_recipient() {
    let (client, id, storage) = client();
    let burn = tests::burn();
    let wallet = wallet(
        burn.source_chain_id,
        burn.sender,
        vec![log(
            burn.source_transmitter,
            cctp::MessageSent {
                message: tests::message(&burn).into(),
            },
        )],
    );
    wallet.fail_receipt.store(true, Ordering::Relaxed);
    assert!(
        client
            .burn_cctp(
                &id,
                &wallet,
                &burn,
                BridgeSubmission::Wallet,
                &PollOptions::default()
            )
            .await
            .is_err()
    );
    let record = client.get_stored_swap(&id).unwrap().unwrap();
    let operation = &record.metadata.bridge_operations.unwrap()["cctp_burn"];
    assert!(operation.transaction_hash.is_some());
    assert!(!operation.complete);
    wallet.fail_receipt.store(false, Ordering::Relaxed);
    let restarted = Client::builder()
        .base_url("http://localhost")
            .mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about")
        .storage(storage)
        .build()
        .unwrap();
    restarted
        .burn_cctp(
            &id,
            &wallet,
            &burn,
            BridgeSubmission::Wallet,
            &PollOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(wallet.calls.lock().unwrap().len(), 1);
    let mut changed = burn.clone();
    changed.mint_recipient = B256::repeat_byte(5);
    assert!(
        restarted
            .burn_cctp(
                &id,
                &wallet,
                &changed,
                BridgeSubmission::Wallet,
                &PollOptions::default()
            )
            .await
            .is_err()
    );
    assert_eq!(wallet.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn lost_broadcast_response_blocks_resubmission_and_can_be_reconciled() {
    let (client, id, _) = client();
    let burn = tests::burn();
    let wallet = wallet(
        burn.source_chain_id,
        burn.sender,
        vec![log(
            burn.source_transmitter,
            cctp::MessageSent {
                message: tests::message(&burn).into(),
            },
        )],
    );
    wallet.lose_submission.store(true, Ordering::Relaxed);
    assert!(
        client
            .burn_cctp(
                &id,
                &wallet,
                &burn,
                BridgeSubmission::Wallet,
                &PollOptions::default()
            )
            .await
            .is_err()
    );
    wallet.lose_submission.store(false, Ordering::Relaxed);
    assert!(
        client
            .burn_cctp(
                &id,
                &wallet,
                &burn,
                BridgeSubmission::Wallet,
                &PollOptions::default()
            )
            .await
            .is_err()
    );
    assert_eq!(wallet.calls.lock().unwrap().len(), 1);
    client
        .attach_bridge_transaction(&id, "cctp_burn", wallet.receipt.transaction_hash)
        .await
        .unwrap();
    client
        .burn_cctp(
            &id,
            &wallet,
            &burn,
            BridgeSubmission::Wallet,
            &PollOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(wallet.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn mint_matches_event_body_and_skips_an_already_consumed_nonce() {
    let (client, id, _) = client();
    let burn = tests::burn();
    let message = tests::message(&burn);
    let destination = CctpDestination::mainnet(42161).unwrap();
    let attestation:cctp::CctpAttestation=serde_json::from_value(json!({"message":format!("0x{}",hex::encode(&message)),"attestation":"0x1234","status":"complete"})).unwrap();
    let owner = Address::repeat_byte(2);
    let mut wallet = wallet(
        42161,
        owner,
        vec![log(
            destination.transmitter,
            cctp::MessageReceived {
                caller: owner,
                sourceDomain: burn.source_domain,
                nonce: B256::repeat_byte(7),
                sender: burn.source_messenger.into_word(),
                finalityThresholdExecuted: 1000,
                messageBody: message[148..].to_vec().into(),
            },
        )],
    );
    wallet.used_nonce = true;
    let mint = CctpMint {
        burn: &burn,
        destination: &destination,
        attestation: &attestation,
    };
    assert!(
        client
            .mint_cctp(
                &id,
                &wallet,
                mint.clone(),
                BridgeSubmission::Wallet,
                &PollOptions::default()
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(wallet.calls.lock().unwrap().is_empty());
    wallet.used_nonce = false;
    client
        .mint_cctp(
            &id,
            &wallet,
            mint.clone(),
            BridgeSubmission::Wallet,
            &PollOptions::default(),
        )
        .await
        .unwrap()
        .unwrap();
    client
        .mint_cctp(
            &id,
            &wallet,
            mint,
            BridgeSubmission::Wallet,
            &PollOptions::default(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wallet.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn oft_send_bounds_native_fee_and_recovers_from_persisted_guid() {
    let (client, id, _) = client();
    let mut transfer = usdt0::OftTransfer {
        chain_id: 42161,
        source_eid: 30110,
        destination_eid: 30111,
        token: Address::repeat_byte(1),
        oft: usdt0::ARBITRUM_OFT,
        sender: Address::repeat_byte(2),
        recipient: Address::repeat_byte(3).into_word(),
        amount: U256::from(10_000_000),
        minimum_received: U256::from(9_999_000),
        max_native_fee: U256::from(11),
    };
    let wallet = wallet(
        42161,
        transfer.sender,
        vec![log(
            transfer.oft,
            usdt0::OFTSent {
                guid: B256::repeat_byte(4),
                dstEid: transfer.destination_eid,
                fromAddress: transfer.sender,
                amountSentLD: transfer.amount,
                amountReceivedLD: transfer.amount,
            },
        )],
    );
    assert!(
        client
            .send_usdt0(
                &id,
                &wallet,
                &transfer,
                BridgeSubmission::Wallet,
                &PollOptions::default()
            )
            .await
            .is_err()
    );
    assert!(wallet.calls.lock().unwrap().is_empty());
    transfer.max_native_fee = U256::from(12);
    let (_, guid) = client
        .send_usdt0(
            &id,
            &wallet,
            &transfer,
            BridgeSubmission::Wallet,
            &PollOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(guid, B256::repeat_byte(4));
    let (_, resumed) = client
        .send_usdt0(
            &id,
            &wallet,
            &transfer,
            BridgeSubmission::Wallet,
            &PollOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(guid, resumed);
    assert_eq!(wallet.calls.lock().unwrap().len(), 1);
    assert_eq!(wallet.calls.lock().unwrap()[0].value, U256::from(12));
}

#[tokio::test]
async fn sponsored_cctp_mint_and_funding_share_one_batch_and_resume_after_restart() {
    use crate::aa::AaConfig;
    use crate::aa::UserOpConfig;
    use crate::aa::abi::Call;
    use crate::aa::abi::IERC20;
    use crate::aa::abi::IHTLCCoordinator;
    use crate::aa::abi::IKernel;
    use crate::evm::FundingOptions;
    use crate::evm::funding::SwapCreated;
    use crate::evm::htlc::EvmHtlc;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let (_, id, storage) = client();
    let server = MockServer::start().await;
    let sdk=Client::builder().base_url(server.uri()).mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").storage(storage.clone()).build().unwrap();
    let owner: Address = sdk
        .signer
        .as_ref()
        .unwrap()
        .derive_evm_key(0)
        .unwrap()
        .address
        .parse()
        .unwrap();
    let mut burn = tests::burn();
    burn.mint_recipient = owner.into_word();
    let destination = CctpDestination::mainnet(42161).unwrap();
    let coordinator = Address::repeat_byte(22);
    let htlc = EvmHtlc {
        chain_id: 42161,
        address: Address::repeat_byte(23),
        hash_lock: B256::from(
            sdk.signer
                .as_ref()
                .unwrap()
                .derive_swap_params(0)
                .unwrap()
                .hash_lock,
        ),
        amount: U256::from(100_000),
        token: Address::repeat_byte(25),
        sender: coordinator,
        claimer: Address::repeat_byte(26),
        timelock: 2_000_000_000,
    };
    let mut response =
        serde_json::to_value(storage.get_swap(&id).unwrap().unwrap().response).unwrap();
    response["evm_chain_id"] = json!(42161);
    response["evm_htlc_address"] = json!(htlc.address);
    response["hash_lock"] = json!(htlc.hash_lock);
    response["client_evm_address"] = json!(owner);
    response["server_evm_address"] = json!(htlc.claimer);
    response["evm_refund_locktime"] = json!(htlc.timelock);
    response["source_token"]["token_id"] = json!(destination.usdc);
    let mut foreign = response.clone();
    foreign["hash_lock"] = json!(B256::repeat_byte(24));
    storage
        .store_swap(
            StoredSwap::new(
                0,
                serde_json::from_value(foreign).unwrap(),
                SwapMetadata::default(),
            )
            .unwrap(),
        )
        .unwrap();
    let wallet = wallet(
        42161,
        owner,
        vec![log(
            htlc.address,
            SwapCreated {
                preimageHash: htlc.hash_lock,
                refundAddress: coordinator,
                claimAddress: htlc.claimer,
                token: htlc.token,
                amount: htlc.amount,
                timelock: U256::from(htlc.timelock),
                key: htlc.key(),
            },
        )],
    );
    let calls: Vec<Call> = vec![];
    let payload = json!({"coordinator_address":coordinator,"permit2_address":crate::aa::permit2::PERMIT2_ADDRESS,"source_token_address":destination.usdc,"source_amount":"9999000","lock_token_address":htlc.token,"preimage_hash":htlc.hash_lock,"claim_address":htlc.claimer,"timelock":htlc.timelock,"calls":[],"calls_hash":alloy::primitives::keccak256(calls.abi_encode())});
    Mock::given(method("GET"))
        .and(path(format!("/swap/{id}/swap-and-lock-calldata-permit2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(payload))
        .expect(1)
        .mount(&server)
        .await;
    let operation = Arc::new(Mutex::new(None));
    let submitted = operation.clone();
    let txid = wallet.receipt.transaction_hash;
    let persisted = storage.clone();
    let swap_id = id.clone();
    Mock::given(method("POST")).respond_with(move |request:&wiremock::Request| {
        let body:serde_json::Value=request.body_json().unwrap();
        let result=match body["method"].as_str().unwrap() {
            "eth_chainId"=>json!("0xa4b1"),"eth_getTransactionCount"=>json!("0x3"),"eth_call"=>json!(format!("0x{:064x}",0)),"eth_gasPrice"=>json!("0x100000"),
            "eth_estimateUserOperationGas"=>json!({"callGasLimit":"0x70000","verificationGasLimit":"0x20000","preVerificationGas":"0x10000"}),
            "eth_sendUserOperation"=>{
                let encoded:Bytes=body["params"][0]["callData"].as_str().unwrap().parse().unwrap();
                let execute=IKernel::executeCall::abi_decode(&encoded).unwrap();
                let calls=Vec::<Call>::abi_decode(&execute.executionCalldata).unwrap();
                assert_eq!(calls.len(),4);
                assert_eq!(calls[0].target,cctp::MESSAGE_TRANSMITTER);assert!(calls[0].callData.starts_with(&cctp::receiveMessageCall::SELECTOR));
                let reset=IERC20::approveCall::abi_decode(&calls[1].callData).unwrap();assert_eq!(reset.amount,U256::ZERO);
                let approval=IERC20::approveCall::abi_decode(&calls[2].callData).unwrap();assert_eq!(approval.amount,U256::from(9_999_000));
                let funding=IHTLCCoordinator::executeAndCreateWithPermit2Call::abi_decode(&calls[3].callData).unwrap();
                assert_eq!(funding.depositor,owner);assert_eq!(funding.permit.permitted.amount,approval.amount);
                assert!(!funding.signature.is_empty());
                assert!(persisted.get_swap(&swap_id).unwrap().unwrap().metadata.bridge_operations.unwrap().contains_key("cctp_fund"));
                let operation=crate::aa::orchestrate::test_support::submitted_user_op_hash(&body,42161);
                *submitted.lock().unwrap()=Some(operation);
                json!(operation)
            },
            "eth_getUserOperationReceipt"=>{
                let operation=submitted.lock().unwrap().unwrap();
                assert_eq!(persisted.get_swap(&swap_id).unwrap().unwrap().metadata.bridge_operations.unwrap()["cctp_fund"].user_operation_hash,Some(operation.to_string()));
                json!({"success":true,"receipt":{"transactionHash":txid}})
            },
            "eth_getTransactionReceipt"=>{
                let operation=submitted.lock().unwrap().unwrap();
                crate::aa::orchestrate::test_support::node_receipt(txid,&[crate::aa::orchestrate::test_support::user_operation_log(Address::repeat_byte(12),operation,true)])
            },other=>panic!("unexpected RPC {other}"),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
    }).mount(&server).await;
    let config = UserOpConfig {
        entry_point: Address::repeat_byte(12),
        delegation_target: Address::repeat_byte(13),
        aa: AaConfig {
            node_rpc_url: server.uri().parse().unwrap(),
            bundler_url: server.uri().parse().unwrap(),
            paymaster: None,
            bundler_casing: Default::default(),
            gas_overrides: None,
        },
    };
    let attestation:cctp::CctpAttestation=serde_json::from_value(json!({"message":format!("0x{}",hex::encode(tests::message(&burn))),"attestation":"0x1234","status":"complete"})).unwrap();
    let request = CctpFunding {
        mint: CctpMint {
            burn: &burn,
            destination: &destination,
            attestation: &attestation,
        },
        limits: FundingOptions {
            coordinator,
            htlc: htlc.address,
            lock_token: htlc.token,
            max_source_amount: U256::from(9_999_900),
            allowed_call_targets: vec![],
            timeout: std::time::Duration::from_secs(10),
        },
        account: &config,
    };
    let rejected = sdk
        .fund_swap_from_cctp(&id, &wallet, request.clone(), &PollOptions::default())
        .await
        .unwrap_err();
    assert!(rejected.to_string().contains("seed-derived"), "{rejected}");
    assert!(
        !storage
            .get_swap(&id)
            .unwrap()
            .unwrap()
            .metadata
            .bridge_operations
            .is_some_and(|operations| operations.contains_key("cctp_fund"))
    );
    storage
        .store_swap(
            StoredSwap::new(
                0,
                serde_json::from_value(response).unwrap(),
                SwapMetadata::default(),
            )
            .unwrap(),
        )
        .unwrap();
    wallet.now.store(htlc.timelock, Ordering::Relaxed);
    let expired = sdk
        .fund_swap_from_cctp(&id, &wallet, request.clone(), &PollOptions::default())
        .await
        .unwrap_err();
    assert!(expired.to_string().contains("timelock has expired"));
    assert!(
        storage
            .get_swap(&id)
            .unwrap()
            .unwrap()
            .metadata
            .bridge_operations
            .is_none_or(|operations| !operations.contains_key("cctp_fund"))
    );
    wallet.now.store(htlc.timelock - 1, Ordering::Relaxed);
    sdk.fund_swap_from_cctp(&id, &wallet, request.clone(), &PollOptions::default())
        .await
        .unwrap();
    // A batch that already landed must still resume once the timelock has passed.
    wallet.now.store(htlc.timelock + 1, Ordering::Relaxed);
    sdk.fund_swap_from_cctp(&id, &wallet, request, &PollOptions::default())
        .await
        .unwrap();
    assert_eq!(
        storage
            .get_swap(&id)
            .unwrap()
            .unwrap()
            .metadata
            .evm_fund_txid,
        Some(txid.to_string())
    );
}
#[tokio::test]
async fn bridge_resume_records_only_a_transaction_the_node_shows_executing_the_operation() {
    use crate::aa::orchestrate::test_support::user_operation_log;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    let (client, id, _) = client();
    let server = MockServer::start().await;
    let reported = B256::repeat_byte(8);
    Mock::given(method("POST"))
        .respond_with(move |request: &wiremock::Request| {
            let body: serde_json::Value = request.body_json().unwrap();
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":{"success":true,"receipt":{"transactionHash":reported}}}))
        })
        .mount(&server)
        .await;
    let config = crate::aa::UserOpConfig {
        entry_point: Address::repeat_byte(12),
        delegation_target: Address::repeat_byte(13),
        aa: crate::aa::AaConfig {
            node_rpc_url: server.uri().parse().unwrap(),
            bundler_url: server.uri().parse().unwrap(),
            paymaster: None,
            bundler_casing: Default::default(),
            gas_overrides: None,
        },
    };
    let operation = B256::repeat_byte(19);
    let intent = json!({"bridge":"usdt0"});
    client
        .save_bridge_operation(
            &id,
            "usdt0_send",
            crate::storage::BridgeOperation {
                intent: intent.clone(),
                transaction_hash: None,
                user_operation_hash: Some(operation.to_string()),
                complete: false,
            },
        )
        .unwrap();
    let mode = BridgeSubmission::UserOperation(&config);
    let foreign = wallet(42161, Address::repeat_byte(1), vec![]);
    assert!(
        client
            .bridge_receipt(&id, "usdt0_send", &intent, &foreign, &mode)
            .await
            .is_err()
    );
    assert_eq!(
        client
            .bridge_operation(&id, "usdt0_send")
            .unwrap()
            .unwrap()
            .transaction_hash,
        None
    );
    let ours = wallet(
        42161,
        Address::repeat_byte(1),
        vec![user_operation_log(config.entry_point, operation, true)],
    );
    let receipt = client
        .bridge_receipt(&id, "usdt0_send", &intent, &ours, &mode)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.transaction_hash, reported);
    assert_eq!(
        client
            .bridge_operation(&id, "usdt0_send")
            .unwrap()
            .unwrap()
            .transaction_hash,
        Some(reported.to_string())
    );
}
#[tokio::test]
async fn sweep_can_run_again_after_completion() {
    let (client, id, _) = client();
    let owner = Address::repeat_byte(1);
    let token = Address::repeat_byte(2);
    let recipient = Address::repeat_byte(3);
    let transfer = |value: u64| {
        log(
            token,
            execution::Transfer {
                from: owner,
                to: recipient,
                value: U256::from(value),
            },
        )
    };
    let wallet = wallet(137, owner, vec![transfer(10), transfer(20), transfer(30)]);
    let sweep = |amount: u64| BridgeRecovery {
        chain_id: 137,
        token,
        recipient,
        amount: U256::from(amount),
    };
    for amount in [10, 20, 20] {
        client
            .recover_bridge_balance(
                &id,
                &wallet,
                &sweep(amount),
                BridgeSubmission::Wallet,
                &PollOptions::default(),
            )
            .await
            .unwrap();
    }
    assert_eq!(wallet.calls.lock().unwrap().len(), 2);
    wallet.lose_submission.store(true, Ordering::Relaxed);
    for _ in 0..2 {
        assert!(
            client
                .recover_bridge_balance(
                    &id,
                    &wallet,
                    &sweep(30),
                    BridgeSubmission::Wallet,
                    &PollOptions::default(),
                )
                .await
                .is_err()
        );
    }
    assert_eq!(wallet.calls.lock().unwrap().len(), 3);
}
