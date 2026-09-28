use super::cctp::*;
use super::execution;
use super::execution_tests::Wallet;
use super::execution_tests::log;
use super::execution_tests::wallet;
use super::recover::*;
use super::tests;
use super::*;
use crate::Client;
use crate::Error;
use crate::InMemorySwapStorage;
use crate::StoredSwap;
use crate::SwapMetadata;
use crate::SwapStorage;
use crate::control::PollOptions;
use crate::evm::*;
use crate::storage::BridgeOperation;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolValue;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const SOURCE_TX: B256 = B256::repeat_byte(4);
const RECIPIENT: Address = Address::repeat_byte(0x33);
const MINTED: u64 = 9_999_900;

fn seed_client(storage: Arc<dyn SwapStorage>) -> Client {
    Client::builder()
        .base_url("http://localhost")
        .mnemonic(MNEMONIC)
        .storage(storage)
        .build()
        .unwrap()
}
fn address(key: crate::signer::EvmKey) -> Address {
    key.address.parse().unwrap()
}
fn wallet_account(client: &Client) -> Address {
    address(
        client
            .signer
            .as_ref()
            .unwrap()
            .derive_wallet_evm_key()
            .unwrap(),
    )
}
fn burn_to(account: Address) -> CctpBurn {
    CctpBurn {
        mint_recipient: account.into_word(),
        ..tests::burn()
    }
}
/// As emitted on the source chain: Circle fills in the nonce, executed finality,
/// executed fee and expiry when it attests.
fn source_message(burn: &CctpBurn) -> Vec<u8> {
    let mut message = tests::message(burn);
    message[12..44].fill(0);
    message[144..148].fill(0);
    message[312..376].fill(0);
    message
}
fn deposit(burn: &CctpBurn) -> DepositForBurn {
    DepositForBurn {
        burnToken: burn.burn_token,
        amount: burn.amount,
        depositor: burn.sender,
        mintRecipient: burn.mint_recipient,
        destinationDomain: burn.destination_domain,
        destinationTokenMessenger: burn.destination_messenger,
        destinationCaller: burn.destination_caller,
        maxFee: burn.max_fee,
        minFinalityThreshold: burn.min_finality,
        hookData: burn.hook.clone(),
    }
}
fn sent(burn: &CctpBurn) -> EvmLog {
    log(
        burn.source_transmitter,
        MessageSent {
            message: source_message(burn).into(),
        },
    )
}
fn source_logs(burn: &CctpBurn) -> Vec<EvmLog> {
    vec![sent(burn), log(burn.source_messenger, deposit(burn))]
}
fn receipt(logs: Vec<EvmLog>) -> EvmReceipt {
    EvmReceipt {
        transaction_hash: SOURCE_TX,
        success: true,
        block_number: 1,
        logs,
    }
}
fn attestation(burn: &CctpBurn) -> CctpAttestation {
    serde_json::from_value(tests::attested(burn)).unwrap()
}
fn mint_log(burn: &CctpBurn) -> EvmLog {
    log(
        CctpDestination::mainnet(42161).unwrap().transmitter,
        MessageReceived {
            caller: Address::from_word(burn.mint_recipient),
            sourceDomain: burn.source_domain,
            nonce: B256::repeat_byte(7),
            sender: burn.source_messenger.into_word(),
            finalityThresholdExecuted: 1000,
            messageBody: tests::message(burn)[148..].to_vec().into(),
        },
    )
}
fn transfer_log(from: Address, to: Address, amount: u64) -> EvmLog {
    log(
        CctpDestination::mainnet(42161).unwrap().usdc,
        execution::Transfer {
            from,
            to,
            value: U256::from(amount),
        },
    )
}
/// The mint recipient's account on the destination chain. It holds no USDC until
/// its mint transaction is sent, then the net minted amount. Every submission's
/// receipt carries `logs`.
fn account_wallet(burn: &CctpBurn, logs: Vec<EvmLog>) -> Wallet {
    let mut wallet = wallet(42161, Address::from_word(burn.mint_recipient), logs);
    wallet.balance = U256::ZERO;
    wallet.mint_credit = U256::from(MINTED);
    wallet
}
/// Every submission's receipt carries the mint event and a transfer of `amount`
/// to `RECIPIENT`.
fn destination_wallet(burn: &CctpBurn, amount: u64) -> Wallet {
    let account = Address::from_word(burn.mint_recipient);
    account_wallet(
        burn,
        vec![mint_log(burn), transfer_log(account, RECIPIENT, amount)],
    )
}
/// Journal key of the test burn, plus a suffix for wallet submissions.
fn journal(suffix: &str) -> String {
    format!("cctp_recover:6:{}{suffix}", B256::repeat_byte(7))
}
fn user_op_config(url: &str) -> crate::aa::UserOpConfig {
    crate::aa::UserOpConfig {
        entry_point: Address::repeat_byte(12),
        delegation_target: Address::repeat_byte(13),
        aa: crate::aa::AaConfig {
            node_rpc_url: url.parse().unwrap(),
            bundler_url: url.parse().unwrap(),
            paymaster: None,
            bundler_casing: Default::default(),
            gas_overrides: None,
        },
    }
}
type Batch = (Vec<crate::aa::abi::Call>, bool);
/// A node and bundler that accept every UserOperation, mined in `txid`. Records
/// each batch's calls and whether the burn's journal entry existed when it was sent.
struct Bundler {
    server: MockServer,
    batches: Arc<Mutex<Vec<Batch>>>,
    submitted: Arc<Mutex<Option<B256>>>,
}
impl Bundler {
    async fn start(storage: Arc<InMemorySwapStorage>, txid: B256) -> Self {
        use crate::aa::abi::IKernel;
        use crate::aa::orchestrate::test_support::*;
        let server = MockServer::start().await;
        let batches = Arc::new(Mutex::new(Vec::new()));
        let submitted = Arc::new(Mutex::new(None));
        let (recorded, seen) = (batches.clone(), submitted.clone());
        Mock::given(method("POST"))
            .respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value = request.body_json().unwrap();
                let result = match body["method"].as_str().unwrap() {
                    "eth_chainId" => json!("0xa4b1"),
                    "eth_getTransactionCount" => json!("0x3"),
                    "eth_call" => json!(format!("0x{:064x}", 0)),
                    "eth_gasPrice" => json!("0x100000"),
                    "eth_estimateUserOperationGas" => json!({"callGasLimit":"0x70000","verificationGasLimit":"0x20000","preVerificationGas":"0x10000"}),
                    "eth_sendUserOperation" => {
                        let encoded: Bytes =
                            body["params"][0]["callData"].as_str().unwrap().parse().unwrap();
                        let execute = IKernel::executeCall::abi_decode(&encoded).unwrap();
                        let calls =
                            Vec::<crate::aa::abi::Call>::abi_decode(&execute.executionCalldata)
                                .unwrap();
                        let journaled = storage.get_bridge_journal(&journal("")).unwrap().is_some();
                        recorded.lock().unwrap().push((calls, journaled));
                        let operation = submitted_user_op_hash(&body, 42161);
                        *seen.lock().unwrap() = Some(operation);
                        json!(operation)
                    }
                    "eth_getUserOperationReceipt" => {
                        json!({"success":true,"receipt":{"transactionHash":txid}})
                    }
                    "eth_getTransactionReceipt" => {
                        let operation = seen.lock().unwrap().unwrap();
                        node_receipt(txid, &[user_operation_log(Address::repeat_byte(12), operation, true)])
                    }
                    other => panic!("unexpected RPC {other}"),
                };
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
            })
            .mount(&server)
            .await;
        Self {
            server,
            batches,
            submitted,
        }
    }
    fn config(&self) -> crate::aa::UserOpConfig {
        user_op_config(&self.server.uri())
    }
    fn batches(&self) -> Vec<Batch> {
        self.batches.lock().unwrap().clone()
    }
}
/// A copy of the fixture swap owned by `owner`.
fn swap_owned_by(owner: Address, index: u32, metadata: SwapMetadata) -> StoredSwap {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let mut response = fixtures["evm_to_arkade"].clone();
    response["id"] = json!(format!("00000000-0000-0000-0000-{:012}", 100 + index));
    response["client_evm_address"] = json!(owner);
    StoredSwap::new(index, serde_json::from_value(response).unwrap(), metadata).unwrap()
}
fn client_with(swaps: Vec<StoredSwap>) -> Client {
    let storage = InMemorySwapStorage::shared();
    for swap in swaps {
        storage.store_swap(swap).unwrap();
    }
    seed_client(storage)
}
/// One bridge operation under `key`, for the burn with `nonce` if given.
fn operation(key: &str, nonce: Option<B256>, complete: bool) -> SwapMetadata {
    let intent = match nonce {
        Some(nonce) => json!({"nonce":nonce}),
        None => json!({}),
    };
    SwapMetadata {
        bridge_operations: Some(
            [(
                key.to_string(),
                BridgeOperation {
                    intent,
                    transaction_hash: None,
                    user_operation_hash: None,
                    complete,
                },
            )]
            .into(),
        ),
        ..Default::default()
    }
}
fn local() -> CctpRecoveryAccount<'static> {
    CctpRecoveryAccount::Local {
        destination_rpc: "http://localhost:1".parse().unwrap(),
        key_index: None,
    }
}
fn request<'a>(
    burn: &CctpBurn,
    account: CctpRecoveryAccount<'a>,
    submission: BridgeSubmission<'a>,
) -> CctpInboundRecovery<'a> {
    CctpInboundRecovery {
        source: CctpSource::mainnet(8453).unwrap(),
        source_rpc: "http://localhost:1".parse().unwrap(),
        source_txid: SOURCE_TX,
        destination: CctpDestination::mainnet(42161).unwrap(),
        recipient: RECIPIENT,
        account,
        submission,
        sweep: CctpSweep::Minted,
        attestation: Some(attestation(burn)),
    }
}
/// The public method minus its two RPC connections.
async fn recover(
    client: &Client,
    recovery: &CctpInboundRecovery<'_>,
    source: &EvmReceipt,
    wallet: &Wallet,
) -> crate::Result<CctpInboundRecovered> {
    let api = BridgeApi::new("http://localhost:1", "http://localhost:1").unwrap();
    let selected = client.select_recovery(recovery, source)?;
    client
        .execute_recovery(&api, recovery, selected, wallet, &PollOptions::default())
        .await
}
fn sent_calls(wallet: &Wallet) -> Vec<EvmTransaction> {
    wallet.calls.lock().unwrap().clone()
}
fn transfer_amount(tx: &EvmTransaction) -> U256 {
    execution::Token::transferCall::abi_decode(&tx.data)
        .unwrap()
        .amount
}

#[test]
fn burns_from_receipt_rebuilds_terms_and_rejects_forged_logs() {
    // Base -> Optimism burn from mainnet transaction
    // 0xfb773d2d890d961b0c408101d1e769df4c9c865c2b47d4e97251ae44a6567ed8.
    let base = CctpSource::mainnet(8453).unwrap();
    let hex = |s: &str| -> Bytes { s.parse().unwrap() };
    let word = |s: &str| -> B256 { s.parse().unwrap() };
    let live = receipt(vec![
        EvmLog {
            address: base.transmitter,
            topics: vec![word(
                "0x8c5261668696ce22758910d05bab8f186d6eb247ceac2af2e82c7dc17669b036",
            )],
            data: hex(concat!(
                "0x",
                "0000000000000000000000000000000000000000000000000000000000000020",
                "0000000000000000000000000000000000000000000000000000000000000178",
                "0000000100000006000000020000000000000000000000000000000000000000",
                "00000000000000000000000000000000000000000000000028b5a0e9c621a5ba",
                "daa536219b3a228c8168cf5d00000000000000000000000028b5a0e9c621a5ba",
                "daa536219b3a228c8168cf5d0000000000000000000000000000000000000000",
                "000000000000000000000000000003e800000000000000010000000000000000",
                "00000000833589fcd6edb6e08f4c7c32d4f71b54bda029130000000000000000",
                "00000000dbe3a5dc79a4e1f171417402e81b8476665e15700000000000000000",
                "000000000000000000000000000000000000000003b8c2400000000000000000",
                "00000000aec53efdc5582dc78f52ed4cfa852bfa5bf0c1d30000000000000000",
                "000000000000000000000000000000000000000000001fb60000000000000000",
                "0000000000000000000000000000000000000000000000000000000000000000",
                "0000000000000000000000000000000000000000000000000000000000000000",
            )),
        },
        EvmLog {
            address: base.messenger,
            topics: vec![
                word("0x0c8c1cbdc5190613ebd485511d4e2812cfa45eecb79d845893331fedad5130a5"),
                word("0x000000000000000000000000833589fcd6edb6e08f4c7c32d4f71b54bda02913"),
                word("0x000000000000000000000000aec53efdc5582dc78f52ed4cfa852bfa5bf0c1d3"),
                word("0x00000000000000000000000000000000000000000000000000000000000003e8"),
            ],
            data: hex(concat!(
                "0x",
                "0000000000000000000000000000000000000000000000000000000003b8c240",
                "000000000000000000000000dbe3a5dc79a4e1f171417402e81b8476665e1570",
                "0000000000000000000000000000000000000000000000000000000000000002",
                "00000000000000000000000028b5a0e9c621a5badaa536219b3a228c8168cf5d",
                "0000000000000000000000000000000000000000000000000000000000000000",
                "0000000000000000000000000000000000000000000000000000000000001fb6",
                "00000000000000000000000000000000000000000000000000000000000000e0",
                "0000000000000000000000000000000000000000000000000000000000000000",
            )),
        },
    ]);
    assert_eq!(
        burns_from_receipt(&live, &base).unwrap(),
        vec![CctpBurn {
            source_chain_id: 8453,
            source_domain: 6,
            destination_domain: 2,
            source_messenger: TOKEN_MESSENGER,
            destination_messenger: TOKEN_MESSENGER.into_word(),
            source_transmitter: MESSAGE_TRANSMITTER,
            burn_token: base.usdc,
            sender: "0xaec53efdc5582dc78f52ed4cfa852bfa5bf0c1d3"
                .parse()
                .unwrap(),
            mint_recipient: word(
                "0x000000000000000000000000dbe3a5dc79a4e1f171417402e81b8476665e1570"
            ),
            destination_caller: B256::ZERO,
            amount: U256::from(62_440_000),
            max_fee: U256::from(8118),
            min_finality: 1000,
            hook: Bytes::new(),
        }]
    );

    let burn = burn_to(Address::repeat_byte(2));
    assert_eq!(
        burns_from_receipt(&receipt(source_logs(&burn)), &base).unwrap(),
        vec![burn.clone()]
    );
    let deposit_log = log(base.messenger, deposit(&burn));
    let foreign_transmitter = log(
        Address::repeat_byte(9),
        MessageSent {
            message: source_message(&burn).into(),
        },
    );
    let mut forged_sender = burn.clone();
    forged_sender.source_messenger = Address::repeat_byte(9);
    let mut more = burn.clone();
    more.amount = U256::from(20_000_000);
    let foreign_deposit = log(Address::repeat_byte(9), deposit(&burn));
    let mismatched_deposit = log(base.messenger, deposit(&more));
    for logs in [
        vec![foreign_transmitter, deposit_log.clone()],
        vec![sent(&forged_sender), deposit_log.clone()],
        vec![sent(&burn)],
        vec![sent(&burn), foreign_deposit],
        vec![sent(&burn), mismatched_deposit],
    ] {
        assert!(
            burns_from_receipt(&receipt(logs), &base)
                .unwrap()
                .is_empty()
        );
    }
    let mut two = source_logs(&burn);
    two.extend(source_logs(&more));
    assert_eq!(
        burns_from_receipt(&receipt(two), &base).unwrap(),
        vec![burn.clone(), more]
    );
    // One deposit pays for one message.
    let doubled = vec![sent(&burn), sent(&burn), deposit_log];
    assert_eq!(
        burns_from_receipt(&receipt(doubled), &base).unwrap().len(),
        1
    );
    let mut failed = receipt(source_logs(&burn));
    failed.success = false;
    assert!(burns_from_receipt(&failed, &base).is_err());
}

#[tokio::test]
async fn recovery_ignores_first_iris_message_not_matching_receipt() {
    let client = seed_client(InMemorySwapStorage::shared());
    let burn = burn_to(wallet_account(&client));
    let mut other = burn.clone();
    other.amount = U256::from(50_000_000);
    let iris = MockServer::start().await;
    Mock::given(path("/v2/messages/6"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"messages":[tests::attested(&other),tests::attested(&burn)]}),
            ),
        )
        .mount(&iris)
        .await;
    let api = BridgeApi::new(&iris.uri(), &iris.uri()).unwrap();
    let mut recovery = request(&burn, local(), BridgeSubmission::Wallet);
    recovery.attestation = None;
    let source = receipt(source_logs(&burn));
    let wallet = destination_wallet(&burn, MINTED);
    let selected = client.select_recovery(&recovery, &source).unwrap();
    client
        .execute_recovery(&api, &recovery, selected, &wallet, &PollOptions::default())
        .await
        .unwrap();
    let calls = sent_calls(&wallet);
    let mint = receiveMessageCall::abi_decode(&calls[0].data).unwrap();
    assert_eq!(mint.message.to_vec(), tests::message(&burn));
    assert_eq!(transfer_amount(&calls[1]), U256::from(MINTED));
}

#[tokio::test]
async fn orphan_recovery_mints_and_sweeps_net_amount_without_swap_record() {
    let client = seed_client(InMemorySwapStorage::shared());
    let account = wallet_account(&client);
    let burn = burn_to(account);
    let mut wallet = destination_wallet(&burn, MINTED);
    wallet.balance = U256::from(100_000_000);
    let recovery = request(&burn, local(), BridgeSubmission::Wallet);
    let recovered = recover(&client, &recovery, &receipt(source_logs(&burn)), &wallet)
        .await
        .unwrap();
    let calls = sent_calls(&wallet);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].to, recovery.destination.transmitter);
    assert!(calls[0].data.starts_with(&receiveMessageCall::SELECTOR));
    assert_eq!(calls[1].to, recovery.destination.usdc);
    // The account holds 100 USDC; only this burn's net mint moves.
    assert_eq!(transfer_amount(&calls[1]), U256::from(MINTED));
    assert_eq!(recovered.recovered_amount, U256::from(MINTED));
    assert_eq!(recovered.account, account);
    assert_eq!(recovered.swap_id, None);
    assert_eq!(recovered.nonce, B256::repeat_byte(7));
    assert_eq!(
        recovered.mint_transaction,
        Some(wallet.receipt.transaction_hash)
    );

    let per_swap = address(client.signer.as_ref().unwrap().derive_evm_key(3).unwrap());
    let burn = burn_to(per_swap);
    let selected = client
        .select_recovery(
            &request(&burn, local(), BridgeSubmission::Wallet),
            &receipt(source_logs(&burn)),
        )
        .unwrap();
    assert_eq!(selected.account, per_swap);
    assert_eq!(
        selected.key.map(address),
        Some(per_swap),
        "found by scanning per-swap keys"
    );
}

#[tokio::test]
async fn consumed_nonce_sweeps_minted_amount_and_requires_balance() {
    let client = seed_client(InMemorySwapStorage::shared());
    let burn = burn_to(wallet_account(&client));
    let mut wallet = destination_wallet(&burn, MINTED);
    wallet.used_nonce = true;
    wallet.balance = U256::from(MINTED - 1);
    let recovery = request(&burn, local(), BridgeSubmission::Wallet);
    let source = receipt(source_logs(&burn));
    let short = recover(&client, &recovery, &source, &wallet)
        .await
        .unwrap_err();
    assert!(
        short.to_string().contains("no longer on the account"),
        "{short}"
    );
    assert!(sent_calls(&wallet).is_empty());
    wallet.balance = U256::from(100_000_000);
    let recovered = recover(&client, &recovery, &source, &wallet).await.unwrap();
    let calls = sent_calls(&wallet);
    assert_eq!(calls.len(), 1);
    assert_eq!(transfer_amount(&calls[0]), U256::from(MINTED));
    assert_eq!(recovered.mint_transaction, None);
}

#[tokio::test]
async fn completed_recovery_replays_without_transfer_and_rejects_new_recipient() {
    let storage = InMemorySwapStorage::shared();
    let client = seed_client(storage.clone());
    let burn = burn_to(wallet_account(&client));
    let wallet = destination_wallet(&burn, MINTED);
    let source = receipt(source_logs(&burn));
    let recovery = request(&burn, local(), BridgeSubmission::Wallet);
    let first = recover(&client, &recovery, &source, &wallet).await.unwrap();
    let restarted = seed_client(storage);
    let replayed = recover(&restarted, &recovery, &source, &wallet)
        .await
        .unwrap();
    assert_eq!(
        replayed.sweep.transaction_hash,
        first.sweep.transaction_hash
    );
    assert_eq!(replayed.mint_transaction, first.mint_transaction);
    let mut elsewhere = request(&burn, local(), BridgeSubmission::Wallet);
    elsewhere.recipient = Address::repeat_byte(0x44);
    assert!(
        recover(&restarted, &elsewhere, &source, &wallet)
            .await
            .is_err()
    );
    assert_eq!(sent_calls(&wallet).len(), 2);
}

#[tokio::test]
async fn lost_submission_blocks_until_attached_or_released() {
    let client = seed_client(InMemorySwapStorage::shared());
    let burn = burn_to(wallet_account(&client));
    let wallet = destination_wallet(&burn, MINTED);
    let source = receipt(source_logs(&burn));
    let recovery = request(&burn, local(), BridgeSubmission::Wallet);
    let mint_key = journal(":mint");
    wallet.lose_submission.store(true, Ordering::Relaxed);
    assert!(recover(&client, &recovery, &source, &wallet).await.is_err());
    wallet.lose_submission.store(false, Ordering::Relaxed);
    let blocked = recover(&client, &recovery, &source, &wallet)
        .await
        .unwrap_err();
    assert!(blocked.to_string().contains(&mint_key), "{blocked}");
    assert_eq!(sent_calls(&wallet).len(), 1);
    client
        .attach_cctp_recovery_transaction(&mint_key, wallet.receipt.transaction_hash)
        .await
        .unwrap();
    recover(&client, &recovery, &source, &wallet).await.unwrap();
    assert_eq!(sent_calls(&wallet).len(), 2, "only the sweep was sent");
    assert!(client.release_cctp_recovery(&mint_key).await.is_err());

    let client = seed_client(InMemorySwapStorage::shared());
    let wallet = destination_wallet(&burn, MINTED);
    wallet.lose_submission.store(true, Ordering::Relaxed);
    assert!(recover(&client, &recovery, &source, &wallet).await.is_err());
    wallet.lose_submission.store(false, Ordering::Relaxed);
    client.release_cctp_recovery(&mint_key).await.unwrap();
    recover(&client, &recovery, &source, &wallet).await.unwrap();
    assert_eq!(sent_calls(&wallet).len(), 3);
}

#[tokio::test]
async fn recorded_swap_funding_refuses_recovery() {
    let seed = seed_client(InMemorySwapStorage::shared());
    let per_swap = address(seed.signer.as_ref().unwrap().derive_evm_key(0).unwrap());
    let external = Address::repeat_byte(0x55);
    let funded = SwapMetadata {
        evm_fund_started: Some(true),
        ..Default::default()
    };
    for owner in [per_swap, external] {
        let burn = burn_to(owner);
        let wallet = destination_wallet(&burn, MINTED);
        let source = receipt(source_logs(&burn));
        let recovery = || {
            let account = if owner == external {
                CctpRecoveryAccount::External(&wallet)
            } else {
                local()
            };
            request(&burn, account, BridgeSubmission::Wallet)
        };
        for (metadata, refusal) in [
            (operation("cctp_fund", None, false), Some("unfinished")),
            (operation("cctp_mint", None, false), Some("unfinished")),
            (funded.clone(), Some("records funding")),
            (SwapMetadata::default(), None),
        ] {
            let stored = swap_owned_by(owner, 0, metadata);
            let id = stored.response.id().to_owned();
            let client = client_with(vec![stored]);
            let result = recover(&client, &recovery(), &source, &wallet).await;
            match refusal {
                Some(refusal) => {
                    let error = result.unwrap_err();
                    assert!(error.to_string().contains(refusal), "{owner}: {error}");
                    assert!(sent_calls(&wallet).is_empty());
                }
                None => assert_eq!(result.unwrap().swap_id, Some(id)),
            }
        }
    }
}

#[tokio::test]
async fn shared_account_refuses_only_fundings_tied_to_this_burn() {
    let seed = seed_client(InMemorySwapStorage::shared());
    let account = wallet_account(&seed);
    let burn = burn_to(account);
    let source = receipt(source_logs(&burn));
    let this_burn = Some(B256::repeat_byte(7));
    let funded = |metadata: SwapMetadata| SwapMetadata {
        evm_fund_txid: Some(B256::repeat_byte(1).to_string()),
        ..metadata
    };
    for (metadata, refusal) in [
        (operation("cctp_fund", None, false), Some("unfinished")),
        (
            operation("cctp_fund", this_burn, true),
            Some("records funding"),
        ),
        (
            funded(operation("cctp_mint", this_burn, true)),
            Some("records funding"),
        ),
        // Funding of another burn, and a mint of this one the swap never spent.
        (
            funded(operation("cctp_fund", Some(B256::repeat_byte(6)), true)),
            None,
        ),
        (operation("cctp_mint", this_burn, true), None),
    ] {
        let client = client_with(vec![
            swap_owned_by(account, 0, metadata),
            swap_owned_by(account, 1, SwapMetadata::default()),
        ]);
        // The burn was minted earlier and the account holds other USDC.
        let mut wallet = destination_wallet(&burn, MINTED);
        wallet.used_nonce = true;
        wallet.balance = U256::from(100_000_000);
        let recovery = request(&burn, local(), BridgeSubmission::Wallet);
        let result = recover(&client, &recovery, &source, &wallet).await;
        match refusal {
            Some(refusal) => {
                let error = result.unwrap_err();
                assert!(error.to_string().contains(refusal), "{error}");
                assert!(sent_calls(&wallet).is_empty());
            }
            None => {
                assert_eq!(result.unwrap().swap_id, None);
                assert_eq!(sent_calls(&wallet).len(), 1);
            }
        }
    }
}

#[tokio::test]
async fn fund_swap_from_cctp_refuses_recovered_burn() {
    let (_, id, storage) = super::execution_tests::client();
    let client = seed_client(storage);
    let burn = burn_to(wallet_account(&client));
    let wallet = destination_wallet(&burn, MINTED);
    let recovery = request(&burn, local(), BridgeSubmission::Wallet);
    recover(&client, &recovery, &receipt(source_logs(&burn)), &wallet)
        .await
        .unwrap();
    let config = user_op_config("http://localhost:1");
    let attestation = attestation(&burn);
    let refused = client
        .fund_swap_from_cctp(
            &id,
            &wallet,
            CctpFunding {
                mint: CctpMint {
                    burn: &burn,
                    destination: &recovery.destination,
                    attestation: &attestation,
                },
                limits: FundingOptions {
                    coordinator: Address::repeat_byte(22),
                    htlc: Address::repeat_byte(23),
                    lock_token: Address::repeat_byte(25),
                    max_source_amount: U256::from(MINTED),
                    allowed_call_targets: vec![],
                    timeout: std::time::Duration::from_secs(10),
                },
                account: &config,
            },
            &PollOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("recovery entry"), "{refused}");
    assert_eq!(sent_calls(&wallet).len(), 2);
}

#[tokio::test]
async fn sponsored_recovery_batches_mint_and_transfer() {
    // The balance is read before the batch mints.
    for (sweep, before, moved) in [
        (CctpSweep::Minted, 0, MINTED),
        (CctpSweep::FullBalance, 5, MINTED + 5),
    ] {
        let storage = InMemorySwapStorage::shared();
        let client = seed_client(storage.clone());
        let burn = burn_to(wallet_account(&client));
        let mut wallet = destination_wallet(&burn, moved);
        wallet.balance = U256::from(before);
        let txid = wallet.receipt.transaction_hash;
        let bundler = Bundler::start(storage.clone(), txid).await;
        let config = bundler.config();
        let mut recovery = request(&burn, local(), BridgeSubmission::UserOperation(&config));
        recovery.sweep = sweep;
        let source = receipt(source_logs(&burn));
        let recovered = recover(&client, &recovery, &source, &wallet).await.unwrap();
        let batches = bundler.batches();
        let [(calls, journaled)] = batches.as_slice() else {
            panic!("one UserOperation expected, got {}", batches.len());
        };
        assert!(journaled, "journaled before sending");
        assert_eq!(calls.len(), 2);
        assert!(calls[0].callData.starts_with(&receiveMessageCall::SELECTOR));
        let transfer = execution::Token::transferCall::abi_decode(&calls[1].callData).unwrap();
        assert_eq!(
            (transfer.to, transfer.amount),
            (RECIPIENT, U256::from(moved))
        );
        assert_eq!(recovered.mint_transaction, Some(txid));
        assert_eq!(recovered.recovered_amount, U256::from(moved));
        let entry = storage.get_bridge_journal(&journal("")).unwrap().unwrap();
        assert!(entry.complete);
        assert_eq!(
            entry.user_operation_hash,
            bundler
                .submitted
                .lock()
                .unwrap()
                .map(|hash| hash.to_string())
        );
        // Wallet transactions never go out in a sponsored recovery.
        assert!(sent_calls(&wallet).is_empty());
        wallet.used_nonce = true;
        recover(&client, &recovery, &source, &wallet).await.unwrap();
        assert_eq!(bundler.batches().len(), 1);
    }
}

#[tokio::test]
async fn external_wallet_rejects_user_operation() {
    let client = seed_client(InMemorySwapStorage::shared());
    let burn = burn_to(Address::repeat_byte(2));
    let external = destination_wallet(&burn, MINTED);
    let config = user_op_config("http://localhost:1");
    let api = BridgeApi::new("http://localhost:1", "http://localhost:1").unwrap();
    // The source RPC is unreachable, so a transport error would mean a request went out.
    let refused = client
        .recover_cctp_inbound(
            &api,
            request(
                &burn,
                CctpRecoveryAccount::External(&external),
                BridgeSubmission::UserOperation(&config),
            ),
            &PollOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(refused, Error::InvalidSwap(_)), "{refused}");
}

#[tokio::test]
async fn uncontrolled_mint_recipient_rejected() {
    let client = seed_client(InMemorySwapStorage::shared());
    let burn = burn_to(Address::repeat_byte(2));
    let source = receipt(source_logs(&burn));
    let foreign = request(&burn, local(), BridgeSubmission::Wallet);
    assert!(matches!(
        client.select_recovery(&foreign, &source),
        Err(Error::InvalidSigner(_))
    ));
    let other = wallet(42161, Address::repeat_byte(3), vec![]);
    let external = request(
        &burn,
        CctpRecoveryAccount::External(&other),
        BridgeSubmission::Wallet,
    );
    assert!(matches!(
        client.select_recovery(&external, &source),
        Err(Error::InvalidSigner(_))
    ));
}

#[tokio::test]
async fn destination_caller_mismatch_blocks_mint_not_consumed_sweep() {
    let client = seed_client(InMemorySwapStorage::shared());
    let mut burn = burn_to(wallet_account(&client));
    burn.destination_caller = Address::repeat_byte(9).into_word();
    let mut wallet = destination_wallet(&burn, MINTED);
    let source = receipt(source_logs(&burn));
    let recovery = request(&burn, local(), BridgeSubmission::Wallet);
    let refused = recover(&client, &recovery, &source, &wallet)
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("destination caller"),
        "{refused}"
    );
    assert!(sent_calls(&wallet).is_empty());
    wallet.used_nonce = true;
    wallet.balance = U256::from(MINTED);
    recover(&client, &recovery, &source, &wallet).await.unwrap();
    assert_eq!(sent_calls(&wallet).len(), 1);
}

#[tokio::test]
async fn store_without_journal_refuses_before_sending() {
    #[derive(Default)]
    struct IndexOnly(InMemorySwapStorage);
    impl SwapStorage for IndexOnly {
        fn next_key_index(&self) -> crate::Result<u32> {
            self.0.next_key_index()
        }
        fn put_swap_key_index(&self, swap_id: &str, key_index: u32) -> crate::Result<()> {
            self.0.put_swap_key_index(swap_id, key_index)
        }
        fn get_swap_key_index(&self, swap_id: &str) -> crate::Result<Option<u32>> {
            self.0.get_swap_key_index(swap_id)
        }
    }
    let client = seed_client(Arc::new(IndexOnly::default()));
    let burn = burn_to(wallet_account(&client));
    let wallet = destination_wallet(&burn, MINTED);
    let recovery = request(&burn, local(), BridgeSubmission::Wallet);
    let refused = recover(&client, &recovery, &receipt(source_logs(&burn)), &wallet)
        .await
        .unwrap_err();
    assert!(matches!(refused, Error::Storage(_)), "{refused}");
    assert!(sent_calls(&wallet).is_empty());
}

#[tokio::test]
async fn full_balance_sweep_moves_the_whole_balance() {
    let client = seed_client(InMemorySwapStorage::shared());
    let burn = burn_to(wallet_account(&client));
    let mut wallet = destination_wallet(&burn, MINTED + 5);
    // Read after the mint lands, so the sweep includes it.
    wallet.balance = U256::from(5);
    let mut recovery = request(&burn, local(), BridgeSubmission::Wallet);
    recovery.sweep = CctpSweep::FullBalance;
    let recovered = recover(&client, &recovery, &receipt(source_logs(&burn)), &wallet)
        .await
        .unwrap();
    assert_eq!(
        transfer_amount(&sent_calls(&wallet)[1]),
        U256::from(MINTED + 5)
    );
    assert_eq!(recovered.recovered_amount, U256::from(MINTED + 5));
}

#[tokio::test]
async fn public_recovery_reads_the_source_receipt_from_its_chain() {
    let client = seed_client(InMemorySwapStorage::shared());
    let external = Address::repeat_byte(0x55);
    let burn = burn_to(external);
    let wallet = destination_wallet(&burn, MINTED);
    let api = BridgeApi::new("http://localhost:1", "http://localhost:1").unwrap();
    for (chain, accepted) in [("0x1", false), ("0x2105", true)] {
        let source = MockServer::start().await;
        let logs = source_logs(&burn);
        Mock::given(method("POST"))
            .respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value = request.body_json().unwrap();
                let result = match body["method"].as_str().unwrap() {
                    "eth_chainId" => json!(chain),
                    "eth_getTransactionReceipt" => {
                        crate::aa::orchestrate::test_support::node_receipt(SOURCE_TX, &logs)
                    }
                    other => panic!("unexpected RPC {other}"),
                };
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
            })
            .mount(&source)
            .await;
        let mut recovery = request(
            &burn,
            CctpRecoveryAccount::External(&wallet),
            BridgeSubmission::Wallet,
        );
        recovery.source_rpc = source.uri().parse().unwrap();
        let result = client
            .recover_cctp_inbound(&api, recovery, &PollOptions::default())
            .await;
        assert_eq!(result.is_ok(), accepted, "{result:?}");
    }
    assert_eq!(sent_calls(&wallet).len(), 2);
}

#[tokio::test]
async fn switching_submission_mode_never_sends_a_second_transfer() {
    let seed = seed_client(InMemorySwapStorage::shared());
    let burn = burn_to(wallet_account(&seed));
    let source = receipt(source_logs(&burn));

    // A finished wallet recovery, then the same burn sponsored.
    let storage = InMemorySwapStorage::shared();
    let client = seed_client(storage.clone());
    let mut wallet = destination_wallet(&burn, MINTED);
    recover(
        &client,
        &request(&burn, local(), BridgeSubmission::Wallet),
        &source,
        &wallet,
    )
    .await
    .unwrap();
    wallet.used_nonce = true;
    wallet.balance = U256::from(100_000_000);
    let bundler = Bundler::start(storage, wallet.receipt.transaction_hash).await;
    let config = bundler.config();
    let sponsored = request(&burn, local(), BridgeSubmission::UserOperation(&config));
    let refused = recover(&client, &sponsored, &source, &wallet)
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains(&journal(":sweep")),
        "{refused}"
    );
    assert!(bundler.batches().is_empty());
    assert_eq!(sent_calls(&wallet).len(), 2);

    // A finished sponsored recovery, then the same burn by wallet.
    let storage = InMemorySwapStorage::shared();
    let client = seed_client(storage.clone());
    let mut wallet = destination_wallet(&burn, MINTED);
    let bundler = Bundler::start(storage, wallet.receipt.transaction_hash).await;
    let config = bundler.config();
    let sponsored = request(&burn, local(), BridgeSubmission::UserOperation(&config));
    recover(&client, &sponsored, &source, &wallet)
        .await
        .unwrap();
    wallet.used_nonce = true;
    wallet.balance = U256::from(100_000_000);
    let refused = recover(
        &client,
        &request(&burn, local(), BridgeSubmission::Wallet),
        &source,
        &wallet,
    )
    .await
    .unwrap_err();
    let named = format!("under {} by", journal(""));
    assert!(refused.to_string().contains(&named), "{refused}");
    assert!(sent_calls(&wallet).is_empty());
    assert_eq!(bundler.batches().len(), 1);
}

#[tokio::test]
async fn completed_wallet_mint_leaves_the_unsent_sweep_open() {
    let storage = InMemorySwapStorage::shared();
    let client = seed_client(storage.clone());
    let account = wallet_account(&client);
    let burn = burn_to(account);
    let source = receipt(source_logs(&burn));
    // The mint lands, but the sweep finds no USDC and sends nothing.
    let mut wallet = destination_wallet(&burn, MINTED);
    wallet.mint_credit = U256::ZERO;
    let short = recover(
        &client,
        &request(&burn, local(), BridgeSubmission::Wallet),
        &source,
        &wallet,
    )
    .await
    .unwrap_err();
    assert!(
        short.to_string().contains("no longer on the account"),
        "{short}"
    );
    assert_eq!(sent_calls(&wallet).len(), 1);
    assert!(
        storage
            .get_bridge_journal(&journal(":mint"))
            .unwrap()
            .unwrap()
            .complete
    );
    assert!(
        storage
            .get_bridge_journal(&journal(":sweep"))
            .unwrap()
            .is_none()
    );

    let elsewhere = Address::repeat_byte(0x44);
    let mut moved = account_wallet(
        &burn,
        vec![
            mint_log(&burn),
            transfer_log(account, elsewhere, MINTED + 5),
        ],
    );
    moved.used_nonce = true;
    moved.balance = U256::from(MINTED + 5);
    let mut changed = request(&burn, local(), BridgeSubmission::Wallet);
    changed.recipient = elsewhere;
    changed.sweep = CctpSweep::FullBalance;
    let recovered = recover(&client, &changed, &source, &moved).await.unwrap();
    let calls = sent_calls(&moved);
    assert_eq!(calls.len(), 1, "only the sweep");
    assert_eq!(transfer_amount(&calls[0]), U256::from(MINTED + 5));
    assert_eq!(
        recovered.mint_transaction,
        Some(wallet.receipt.transaction_hash)
    );
}

#[tokio::test]
async fn sponsored_recovery_sweeps_a_finished_wallet_mint_but_not_an_unresolved_one() {
    let storage = InMemorySwapStorage::shared();
    let client = seed_client(storage.clone());
    let account = wallet_account(&client);
    let burn = burn_to(account);
    let source = receipt(source_logs(&burn));
    let by_wallet = request(&burn, local(), BridgeSubmission::Wallet);
    let mut wallet = destination_wallet(&burn, MINTED);
    wallet.mint_credit = U256::ZERO;
    let txid = wallet.receipt.transaction_hash;
    let bundler = Bundler::start(storage.clone(), txid).await;
    let config = bundler.config();
    let sponsored = request(&burn, local(), BridgeSubmission::UserOperation(&config));

    wallet.lose_submission.store(true, Ordering::Relaxed);
    assert!(
        recover(&client, &by_wallet, &source, &wallet)
            .await
            .is_err()
    );
    wallet.lose_submission.store(false, Ordering::Relaxed);
    let refused = recover(&client, &sponsored, &source, &wallet)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains(&journal(":mint")), "{refused}");
    assert!(bundler.batches().is_empty());

    // The mint is resolved and verified, but the sweep finds no USDC.
    client
        .attach_cctp_recovery_transaction(&journal(":mint"), txid)
        .await
        .unwrap();
    assert!(
        recover(&client, &by_wallet, &source, &wallet)
            .await
            .is_err()
    );
    assert!(
        storage
            .get_bridge_journal(&journal(":mint"))
            .unwrap()
            .unwrap()
            .complete
    );
    assert_eq!(sent_calls(&wallet).len(), 1);

    wallet.used_nonce = true;
    wallet.balance = U256::from(MINTED);
    wallet.receipt.logs = vec![transfer_log(account, RECIPIENT, MINTED)];
    let recovered = recover(&client, &sponsored, &source, &wallet)
        .await
        .unwrap();
    let batches = bundler.batches();
    let [(calls, _)] = batches.as_slice() else {
        panic!("one UserOperation expected, got {}", batches.len());
    };
    assert_eq!(calls.len(), 1, "the transfer alone");
    assert_eq!(recovered.mint_transaction, Some(txid));
    assert_eq!(sent_calls(&wallet).len(), 1);
}

#[tokio::test]
async fn wallet_recovery_verifies_mint_and_transfer_before_completing() {
    let seed = seed_client(InMemorySwapStorage::shared());
    let account = wallet_account(&seed);
    let burn = burn_to(account);
    let source = receipt(source_logs(&burn));
    let recovery = request(&burn, local(), BridgeSubmission::Wallet);

    // The mint's receipt lacks this burn's MessageReceived: no sweep follows.
    let storage = InMemorySwapStorage::shared();
    let client = seed_client(storage.clone());
    let wallet = account_wallet(&burn, vec![transfer_log(account, RECIPIENT, MINTED)]);
    let refused = recover(&client, &recovery, &source, &wallet)
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("approved CCTP mint"),
        "{refused}"
    );
    assert_eq!(sent_calls(&wallet).len(), 1);
    let mint = storage
        .get_bridge_journal(&journal(":mint"))
        .unwrap()
        .unwrap();
    assert!(mint.transaction_hash.is_some() && !mint.complete);
    assert!(
        storage
            .get_bridge_journal(&journal(":sweep"))
            .unwrap()
            .is_none()
    );

    // A consumed nonce whose sweep receipt pays someone else, or another amount.
    for (to, amount) in [
        (Address::repeat_byte(0x44), MINTED),
        (RECIPIENT, MINTED - 1),
    ] {
        let storage = InMemorySwapStorage::shared();
        let client = seed_client(storage.clone());
        let mut wallet = account_wallet(&burn, vec![transfer_log(account, to, amount)]);
        wallet.used_nonce = true;
        wallet.balance = U256::from(MINTED);
        let refused = recover(&client, &recovery, &source, &wallet)
            .await
            .unwrap_err();
        assert!(
            refused.to_string().contains("recovery transfer"),
            "{refused}"
        );
        assert_eq!(sent_calls(&wallet).len(), 1);
        let sweep = storage
            .get_bridge_journal(&journal(":sweep"))
            .unwrap()
            .unwrap();
        assert!(sweep.transaction_hash.is_some() && !sweep.complete);
    }
}

#[tokio::test]
async fn sponsored_recovery_verifies_mint_and_transfer_before_completing() {
    let seed = seed_client(InMemorySwapStorage::shared());
    let account = wallet_account(&seed);
    let burn = burn_to(account);
    let source = receipt(source_logs(&burn));
    for (logs, refusal) in [
        (
            vec![transfer_log(account, RECIPIENT, MINTED)],
            "approved CCTP mint",
        ),
        (
            vec![
                mint_log(&burn),
                transfer_log(account, RECIPIENT, MINTED - 1),
            ],
            "recovery transfer",
        ),
    ] {
        let storage = InMemorySwapStorage::shared();
        let client = seed_client(storage.clone());
        let wallet = account_wallet(&burn, logs);
        let bundler = Bundler::start(storage.clone(), wallet.receipt.transaction_hash).await;
        let config = bundler.config();
        let recovery = request(&burn, local(), BridgeSubmission::UserOperation(&config));
        let refused = recover(&client, &recovery, &source, &wallet)
            .await
            .unwrap_err();
        assert!(refused.to_string().contains(refusal), "{refused}");
        assert_eq!(bundler.batches().len(), 1);
        assert!(
            !storage
                .get_bridge_journal(&journal(""))
                .unwrap()
                .unwrap()
                .complete
        );
    }
}

#[test]
fn sweep_amount_bounds() {
    let minted = U256::from(MINTED);
    let amount = |sweep, balance: u64, minting| {
        super::recover::sweep_amount(sweep, U256::from(balance), minted, minting).ok()
    };
    assert_eq!(amount(CctpSweep::Minted, 0, true), Some(minted));
    assert_eq!(amount(CctpSweep::Minted, MINTED, false), Some(minted));
    assert_eq!(amount(CctpSweep::Minted, MINTED - 1, false), None);
    assert_eq!(
        amount(CctpSweep::FullBalance, 5, true),
        Some(minted + U256::from(5))
    );
    assert_eq!(
        amount(CctpSweep::FullBalance, 5, false),
        Some(U256::from(5))
    );
    assert_eq!(amount(CctpSweep::FullBalance, 0, false), None);
}
