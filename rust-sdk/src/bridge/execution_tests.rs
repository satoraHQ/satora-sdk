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
