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
    high_s: AtomicBool,
    low_signatures: Mutex<Vec<Signature>>,
    missing_receipts: Mutex<Vec<B256>>,
    /// Receipts that never arrive, like a hash that never mined.
    hanging_receipts: Mutex<Vec<B256>>,
    /// Receipts a lagging node serves only after a few seconds.
    slow_receipts: Mutex<Vec<B256>>,
    reverted_receipts: Mutex<Vec<B256>>,
    permit2_allowance: AtomicU64,
    permit_domain: Mutex<B256>,
    permit_nonce: AtomicU64,
    /// Permit2 nonces spent on chain.
    spent_nonces: Arc<Mutex<Vec<U256>>>,
    /// `None` fails the lookup.
    code: Mutex<Option<Bytes>>,
}
fn secp256k1_order() -> U256 {
    U256::from_str_radix(
        "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141",
        16,
    )
    .unwrap()
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
            let low = crate::aa::signing::sign_hash(&self.secret, data.digest)?;
            self.low_signatures.lock().unwrap().push(low);
            if self.high_s.load(Ordering::Relaxed) {
                return Ok(Signature::new(
                    low.r(),
                    secp256k1_order() - low.s(),
                    !low.v(),
                ));
            }
            Ok(low)
        })
    }
    fn call<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, Bytes> {
        Box::pin(async move {
            if tx
                .data
                .starts_with(&crate::bridge::usdt0::quoteSendCall::SELECTOR)
            {
                return Ok(crate::bridge::usdt0::quoteSendCall::abi_encode_returns(
                    &crate::bridge::usdt0::MessagingFee {
                        nativeFee: U256::from(1000),
                        lzTokenFee: U256::ZERO,
                    },
                )
                .into());
            }
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
            } else if tx
                .data
                .starts_with(&super::funding::TokenAllowance::allowanceCall::SELECTOR)
            {
                U256::from(self.permit2_allowance.load(Ordering::Relaxed))
            } else if tx
                .data
                .starts_with(&super::permit::PermitToken::DOMAIN_SEPARATORCall::SELECTOR)
            {
                U256::from_be_bytes(self.permit_domain.lock().unwrap().0)
            } else if tx
                .data
                .starts_with(&super::permit::PermitToken::noncesCall::SELECTOR)
            {
                U256::from(self.permit_nonce.load(Ordering::Relaxed))
            } else if let Ok(query) =
                super::relay::Permit2Nonces::nonceBitmapCall::abi_decode(&tx.data)
            {
                self.spent_nonces
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|nonce| **nonce >> 8_usize == query.wordPosition)
                    .fold(U256::ZERO, |bitmap, nonce| {
                        bitmap | U256::ONE << usize::from(nonce.byte(0))
                    })
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
            if self.hanging_receipts.lock().unwrap().contains(&hash) {
                return std::future::pending().await;
            }
            if self.slow_receipts.lock().unwrap().contains(&hash) {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            if self.missing_receipts.lock().unwrap().contains(&hash) {
                return Err(Error::Timeout("receipt not served".into()));
            }
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
                success: !self.reverted_receipts.lock().unwrap().contains(&hash),
                block_number: 10,
                logs,
            })
        })
    }
    fn block_timestamp(&self) -> WalletFuture<'_, u64> {
        Box::pin(async { Ok(self.now.load(Ordering::Relaxed)) })
    }
    fn code(&self, _: Address) -> WalletFuture<'_, Bytes> {
        Box::pin(async {
            self.code
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| Error::Transport("code lookup failed".into()))
        })
    }
}

/// A real LND hold invoice paying the hash of the preimage at key index 0.
fn invoice_paying_key_index_0() -> String {
    let vectors: Value =
        serde_json::from_str(include_str!("../../../fixtures/lightning-invoices.json")).unwrap();
    vectors["invoices"]["lnd_hold_index_0"]["bolt11"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn setup(claim: bool) -> (Client, Wallet, Arc<InMemorySwapStorage>) {
    setup_kind(claim, EvmHtlcKind::Erc20)
}
fn setup_kind(claim: bool, kind: EvmHtlcKind) -> (Client, Wallet, Arc<InMemorySwapStorage>) {
    setup_fixture(
        if claim {
            "arkade_to_evm"
        } else {
            "evm_to_arkade"
        },
        claim,
        kind,
    )
}
fn setup_fixture(
    name: &str,
    claim: bool,
    kind: EvmHtlcKind,
) -> (Client, Wallet, Arc<InMemorySwapStorage>) {
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
    let r = &mut fixture[name];
    r["id"] = json!("test");
    r["evm_chain_id"] = json!(137);
    r["evm_htlc_address"] = json!(htlc.address);
    r["client_evm_address"] = json!(owner);
    r["server_evm_address"] = json!(server);
    let hash_field = if r.get("evm_hash_lock").is_some() {
        "evm_hash_lock"
    } else {
        "hash_lock"
    };
    r[hash_field] = json!(htlc.hash_lock);
    if r.get("client_lightning_invoice").is_some() {
        r["client_lightning_invoice"] = json!(invoice_paying_key_index_0());
    }
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
        high_s: AtomicBool::new(false),
        low_signatures: Mutex::new(vec![]),
        missing_receipts: Mutex::new(vec![]),
        hanging_receipts: Mutex::new(vec![]),
        slow_receipts: Mutex::new(vec![]),
        reverted_receipts: Mutex::new(vec![]),
        permit2_allowance: AtomicU64::new(0),
        permit_domain: Mutex::new(B256::ZERO),
        permit_nonce: AtomicU64::new(0),
        spent_nonces: Arc::default(),
        code: Mutex::new(None),
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

/// A wallet other than the swap owner, reporting `depositor` from the coordinator.
struct Submitter<'a> {
    wallet: &'a Wallet,
    depositor: Address,
}
impl EvmWallet for Submitter<'_> {
    fn address(&self) -> Address {
        Address::repeat_byte(7)
    }
    fn chain_id(&self) -> u64 {
        self.wallet.chain_id()
    }
    fn sign_typed_data<'a>(&'a self, data: &'a EvmTypedData) -> WalletFuture<'a, Signature> {
        self.wallet.sign_typed_data(data)
    }
    fn call<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, Bytes> {
        if tx.data.starts_with(&Coordinator::depositsCall::SELECTOR) {
            let word = U256::from_be_slice(self.depositor.as_slice()).to_be_bytes::<32>();
            return Box::pin(async move { Ok(word.to_vec().into()) });
        }
        self.wallet.call(tx)
    }
    fn send_transaction<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, B256> {
        self.wallet.send_transaction(tx)
    }
    fn wait_for_receipt(&self, hash: B256) -> WalletFuture<'_, EvmReceipt> {
        self.wallet.wait_for_receipt(hash)
    }
    fn block_timestamp(&self) -> WalletFuture<'_, u64> {
        self.wallet.block_timestamp()
    }
}

#[tokio::test]
async fn coordinator_refunds_accept_any_submitter_for_the_recorded_depositor_only() {
    let (client, wallet, store) = setup(false);
    wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    let foreign = Submitter {
        wallet: &wallet,
        depositor: Address::repeat_byte(8),
    };
    let error = client
        .refund_evm_with_wallet("test", &foreign, options.clone())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("depositor"), "{error}");
    let submitter = Submitter {
        wallet: &wallet,
        depositor: wallet.owner,
    };
    let direct = RefundOptions {
        coordinator: None,
        ..options.clone()
    };
    let error = client
        .refund_evm_with_wallet("test", &submitter, direct)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("does not match swap"), "{error}");
    let error = client
        .get_funded_evm_htlc("test", &submitter, &options)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("does not match swap"), "{error}");
    assert!(wallet.sent.lock().unwrap().is_empty());
    let receipt = client
        .refund_evm_with_wallet("test", &submitter, options)
        .await
        .unwrap();
    let sent = wallet.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        (sent[0].from, sent[0].to),
        (submitter.address(), wallet.coordinator)
    );
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

/// A wallet that replaced its broadcast funding: the sent hash mines as `funding_hash`.
struct Replacing<'a>(&'a Wallet);
impl EvmWallet for Replacing<'_> {
    fn address(&self) -> Address {
        self.0.address()
    }
    fn chain_id(&self) -> u64 {
        self.0.chain_id()
    }
    fn sign_typed_data<'a>(&'a self, data: &'a EvmTypedData) -> WalletFuture<'a, Signature> {
        self.0.sign_typed_data(data)
    }
    fn call<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, Bytes> {
        if tx.data.starts_with(&selector("allowance(address,address)")) {
            return Box::pin(async { Ok(U256::MAX.to_be_bytes::<32>().to_vec().into()) });
        }
        self.0.call(tx)
    }
    fn send_transaction<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, B256> {
        self.0.send_transaction(tx)
    }
    fn wait_for_receipt(&self, _: B256) -> WalletFuture<'_, EvmReceipt> {
        self.0.wait_for_receipt(self.0.funding_hash)
    }
    fn block_timestamp(&self) -> WalletFuture<'_, u64> {
        self.0.block_timestamp()
    }
}

#[tokio::test]
async fn a_replaced_funding_records_the_mined_hash_and_refunds() {
    use crate::aa::abi::Call;
    use alloy::sol_types::SolValue;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::path;
    let (_, wallet, fixture) = setup(false);
    let response = fixture.get_swap("test").unwrap().unwrap().response;
    let store = Arc::new(InMemorySwapStorage::new());
    store
        .store_swap(StoredSwap::new(0, response, SwapMetadata::default()).unwrap())
        .unwrap();
    let h = &wallet.htlc;
    let server = MockServer::start().await;
    Mock::given(path("/swap/test/swap-and-lock-calldata-permit2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "coordinator_address": wallet.coordinator,
            "permit2_address": crate::aa::permit2::PERMIT2_ADDRESS,
            "source_token_address": h.token,
            "source_amount": h.amount.to_string(),
            "lock_token_address": h.token,
            "preimage_hash": h.hash_lock,
            "claim_address": h.claimer,
            "timelock": h.timelock,
            "calls": [],
            "calls_hash": alloy::primitives::keccak256(Vec::<Call>::new().abi_encode()),
        })))
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let receipt = client
        .fund_swap_with_wallet(
            "test",
            &Replacing(&wallet),
            FundingOptions {
                coordinator: wallet.coordinator,
                htlc: h.address,
                lock_token: h.token,
                max_source_amount: h.amount,
                allowed_call_targets: vec![],
                timeout: Duration::from_secs(10),
            },
        )
        .await
        .unwrap();
    assert_eq!(receipt.transaction_hash, wallet.funding_hash);
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_fund_txid,
        Some(wallet.funding_hash.to_string())
    );
    wallet.now.store(h.timelock, Ordering::Relaxed);
    client
        .refund_evm_with_wallet(
            "test",
            &wallet,
            RefundOptions {
                htlc: h.address,
                lock_token: h.token,
                coordinator: Some(wallet.coordinator),
                timeout: Duration::from_secs(10),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_recorded_funding_hash_that_never_mined_is_resolved_through_the_server() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::path;
    // The stale hash's receipt never returns, fails at once, reverts, or fails
    // after the hedge but before the server answers.
    for (stale_mode, server_delay) in [("hangs", 0), ("missing", 0), ("reverted", 0), ("late", 2)] {
        let (_, wallet, store) = setup(false);
        let stale = B256::repeat_byte(43);
        match stale_mode {
            "hangs" => wallet.hanging_receipts.lock().unwrap().push(stale),
            "missing" => wallet.missing_receipts.lock().unwrap().push(stale),
            "reverted" => wallet.reverted_receipts.lock().unwrap().push(stale),
            _ => {
                wallet.slow_receipts.lock().unwrap().push(stale);
                wallet.missing_receipts.lock().unwrap().push(stale);
            }
        }
        let mut stored = store.get_swap("test").unwrap().unwrap();
        stored.metadata.evm_fund_txid = Some(stale.to_string());
        store.store_swap(stored.clone()).unwrap();
        let mut response = serde_json::to_value(&stored.response).unwrap();
        assert!(response.get("evm_fund_txid").is_none_or(Value::is_null));
        response["evm_fund_txid"] = json!(wallet.funding_hash);
        let server = MockServer::start().await;
        Mock::given(path("/swap/test"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(response)
                    .set_delay(Duration::from_secs(server_delay)),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .build()
            .unwrap();
        wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
        client
            .refund_evm_with_wallet(
                "test",
                &wallet,
                RefundOptions {
                    htlc: wallet.htlc.address,
                    lock_token: wallet.htlc.token,
                    coordinator: Some(wallet.coordinator),
                    timeout: Duration::from_secs(10),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .get_swap("test")
                .unwrap()
                .unwrap()
                .metadata
                .evm_fund_txid,
            Some(wallet.funding_hash.to_string())
        );
    }
}

#[tokio::test]
async fn a_failed_server_funding_lookup_is_logged() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::path;
    let (_, wallet, store) = setup(false);
    let stale = B256::repeat_byte(44);
    wallet.missing_receipts.lock().unwrap().push(stale);
    let mut stored = store.get_swap("test").unwrap().unwrap();
    stored.metadata.evm_fund_txid = Some(stale.to_string());
    store.store_swap(stored).unwrap();
    let server = MockServer::start().await;
    Mock::given(path("/swap/test"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let mut client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store)
        .build()
        .unwrap();
    let logger = Arc::new(crate::logging::RecordingLogger::default());
    client.set_logger(logger.clone(), crate::logging::LogLevel::Debug);
    wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
    assert!(
        client
            .refund_evm_with_wallet(
                "test",
                &wallet,
                RefundOptions {
                    htlc: wallet.htlc.address,
                    lock_token: wallet.htlc.token,
                    coordinator: Some(wallet.coordinator),
                    timeout: Duration::from_secs(10),
                },
            )
            .await
            .is_err()
    );
    let failed = logger.find("client.refundSwap.serverLookupFailed").unwrap();
    assert_eq!(failed.level, crate::logging::LogLevel::Debug);
    assert_eq!(failed.swap_id.as_deref(), Some("test"));
    assert!(
        failed.data["error"].as_str().unwrap().contains("503"),
        "{failed:?}"
    );
}

#[tokio::test]
async fn a_slow_but_valid_recorded_hash_is_not_held_up_by_a_hung_server() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::path;
    let (_, wallet, store) = setup(false);
    wallet
        .slow_receipts
        .lock()
        .unwrap()
        .push(wallet.funding_hash);
    let server = MockServer::start().await;
    Mock::given(path("/swap/test"))
        .respond_with(ResponseTemplate::new(503).set_delay(Duration::from_secs(20)))
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    client
        .update_swap_metadata(
            "test",
            SwapMetadata {
                evm_fund_txid: Some(wallet.funding_hash.to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
    let started = std::time::Instant::now();
    client
        .refund_evm_with_wallet(
            "test",
            &wallet,
            RefundOptions {
                htlc: wallet.htlc.address,
                lock_token: wallet.htlc.token,
                coordinator: Some(wallet.coordinator),
                timeout: Duration::from_secs(15),
            },
        )
        .await
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_funding_hash_that_never_mined_does_not_stall_the_refund() {
    for stale_is_local in [true, false] {
        let (client, wallet, store) = setup(false);
        let stale = B256::repeat_byte(43);
        wallet.hanging_receipts.lock().unwrap().push(stale);
        let (local, server) = if stale_is_local {
            (stale, wallet.funding_hash)
        } else {
            (wallet.funding_hash, stale)
        };
        client
            .update_swap_metadata(
                "test",
                SwapMetadata {
                    evm_fund_txid: Some(local.to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        rewrite_response(&store, |response| {
            response["evm_fund_txid"] = json!(server);
        });
        wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
        client
            .refund_evm_with_wallet(
                "test",
                &wallet,
                RefundOptions {
                    htlc: wallet.htlc.address,
                    lock_token: wallet.htlc.token,
                    coordinator: Some(wallet.coordinator),
                    timeout: Duration::from_secs(2),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .get_swap("test")
                .unwrap()
                .unwrap()
                .metadata
                .evm_fund_txid,
            Some(wallet.funding_hash.to_string())
        );
    }
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
async fn gasless_claim_outlives_the_client_request_timeout() {
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
        .http(
            reqwest::Client::builder()
                .timeout(Duration::from_millis(200))
                .build()
                .unwrap(),
        )
        .build()
        .unwrap();
    let calls: Vec<crate::aa::abi::Call> = vec![];
    Mock::given(path("/swap/test/redeem-and-swap-calldata")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"min_amount_out":"100000","gasless_fee_sats":0,"calls_hash":keccak256(calls.abi_encode()),"dex_calldata":null}))).mount(&server).await;
    Mock::given(method("POST"))
        .and(path("/swap/test/claim-gasless"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(
                    json!({"id":"test","tx_hash":B256::repeat_byte(9),"message":"submitted"}),
                )
                .set_delay(Duration::from_millis(800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    client
        .claim_evm_gasless("test", &wallet, claim_options(&wallet))
        .await
        .unwrap();
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
async fn gasless_claim_accepted_without_a_hash_keeps_the_claim_reserved() {
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
            json!({"id":"test","tx_hash":"","message":"Gasless claim already in progress"}),
        ))
        .mount(&server)
        .await;
    let submitted = client
        .claim_evm_gasless("test", &wallet, claim_options(&wallet))
        .await
        .unwrap();
    assert!(submitted.tx_hash.is_empty());
    let metadata = store.get_swap("test").unwrap().unwrap().metadata;
    assert_eq!(metadata.evm_claim_txid, None);
    assert_eq!(metadata.evm_claim_started, Some(true));
}

#[tokio::test]
async fn external_high_s_signatures_are_encoded_low_s() {
    use alloy::primitives::keccak256;
    use alloy::sol_types::SolValue;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let half_order = secp256k1_order() >> 1;

    let (client, wallet, _) = setup(true);
    wallet.high_s.store(true, Ordering::Relaxed);
    client
        .claim_evm_with_wallet("test", &wallet, claim_options(&wallet), vec![])
        .await
        .unwrap();
    let low = wallet.low_signatures.lock().unwrap()[0];
    let call = super::claim::ClaimCoordinator::redeemAndExecuteCall::abi_decode(
        &wallet.sent.lock().unwrap()[0].data,
    )
    .unwrap();
    let s = U256::from_be_bytes(call.s.0);
    assert_eq!(s, low.s());
    assert!(s <= half_order);
    assert_eq!(U256::from_be_bytes(call.r.0), low.r());
    assert_eq!(call.v, u8::from(low.v()) + 27);

    let (_, wallet, store) = setup(true);
    wallet.high_s.store(true, Ordering::Relaxed);
    let server = MockServer::start().await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store)
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
    client
        .claim_evm_gasless("test", &wallet, claim_options(&wallet))
        .await
        .unwrap();
    let low = wallet.low_signatures.lock().unwrap()[0];
    let requests = server.received_requests().await.unwrap();
    let body: Value = requests
        .iter()
        .find(|r| r.url.path() == "/swap/test/claim-gasless")
        .unwrap()
        .body_json()
        .unwrap();
    assert!(low.s() <= half_order);
    assert_eq!(body["s"], json!(format!("0x{:064x}", low.s())));
    assert_eq!(body["r"], json!(format!("0x{:064x}", low.r())));
    assert_eq!(body["v"], json!(u8::from(low.v()) + 27));
}

#[tokio::test]
async fn recovery_checks_balance_and_resends_only_for_new_parameters() {
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
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
    options.destination = Address::repeat_byte(9);
    client
        .recover_evm_deposit_with_wallet("test", &wallet, options.clone())
        .await
        .unwrap();
    assert_eq!(wallet.sent.lock().unwrap().len(), 2);
    client
        .recover_evm_deposit_with_wallet("test", &wallet, options)
        .await
        .unwrap();
    assert_eq!(wallet.sent.lock().unwrap().len(), 2);
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
    let (client, mut wallet, store) = setup(false);
    wallet
        .extra_logs
        .push(crate::aa::orchestrate::test_support::user_operation_log(
            crate::aa::userop::ENTRY_POINT_V07,
            B256::repeat_byte(8),
            true,
        ));
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
        let resumed = client
            .resume_gasless_funding("test", &bundler, &wallet)
            .await;
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
async fn funding_resume_rejects_foreign_bundler_transaction() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    let (client, wallet, store) = setup(false);
    let operation = B256::repeat_byte(8).to_string();
    let mut stored = store.get_swap("test").unwrap().unwrap();
    store.delete_swap("test").unwrap();
    stored.metadata.evm_fund_txid = None;
    stored.metadata.evm_fund_userop_hash = Some(operation.clone());
    store.store_swap(stored).unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":{"success":true,"receipt":{"transactionHash":B256::repeat_byte(9)}}}))
        })
        .mount(&server)
        .await;
    let bundler =
        crate::aa::bundler::BundlerClient::new(server.uri().parse().unwrap(), Default::default())
            .unwrap();
    assert!(
        client
            .resume_gasless_funding("test", &bundler, &wallet)
            .await
            .is_err()
    );
    let metadata = store.get_swap("test").unwrap().unwrap().metadata;
    assert_eq!(metadata.evm_fund_txid, None);
    assert_eq!(metadata.evm_fund_userop_hash, Some(operation));
}

#[tokio::test]
async fn claim_resume_records_only_a_transaction_the_node_shows_executing_the_operation() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    let (client, mut wallet, store) = setup(true);
    let operation = B256::repeat_byte(8);
    let mut stored = store.get_swap("test").unwrap().unwrap();
    store.delete_swap("test").unwrap();
    stored.metadata.evm_claim_started = Some(true);
    stored.metadata.evm_claim_userop_hash = Some(operation.to_string());
    store.store_swap(stored).unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":{"success":true,"receipt":{"transactionHash":B256::repeat_byte(9)}}}))
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
    assert!(
        client
            .resume_evm_claim_userop("test", &wallet, &config)
            .await
            .is_err()
    );
    let metadata = store.get_swap("test").unwrap().unwrap().metadata;
    assert_eq!(metadata.evm_claim_txid, None);
    assert_eq!(metadata.evm_claim_userop_hash, Some(operation.to_string()));
    wallet
        .extra_logs
        .push(crate::aa::orchestrate::test_support::user_operation_log(
            config.entry_point,
            operation,
            true,
        ));
    assert_eq!(
        client
            .resume_evm_claim_userop("test", &wallet, &config)
            .await
            .unwrap(),
        Some(B256::repeat_byte(9))
    );
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
    let transaction_hash = B256::repeat_byte(99);
    let sent = Arc::new(Mutex::new(None));
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
                let operation=crate::aa::orchestrate::test_support::submitted_user_op_hash(&body,137);
                *submitted.lock().unwrap()=Some(operation);
                json!(operation)
            },
            "eth_getUserOperationReceipt"=>{
                let operation=submitted.lock().unwrap().unwrap();
                assert_eq!(persisted.get_swap("test").unwrap().unwrap().metadata.evm_claim_userop_hash,Some(operation.to_string()));
                json!({"success":true,"receipt":{"transactionHash":transaction_hash}})
            },
            "eth_getTransactionReceipt"=>{
                let operation=submitted.lock().unwrap().unwrap();
                crate::aa::orchestrate::test_support::node_receipt(transaction_hash,&[crate::aa::orchestrate::test_support::user_operation_log(Address::repeat_byte(12),operation,true)])
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

fn arbitrum_claim() -> (Client, Wallet, Arc<InMemorySwapStorage>) {
    let (client, mut wallet, store) = setup(true);
    wallet.htlc.chain_id = 42161;
    let mut record = store.get_swap("test").unwrap().unwrap();
    let mut response = serde_json::to_value(&record.response).unwrap();
    response["evm_chain_id"] = json!(42161);
    record.response = serde_json::from_value(response).unwrap();
    store.store_swap(record).unwrap();
    (client, wallet, store)
}
fn outbound_dex_call() -> crate::types::DexCall {
    crate::types::DexCall {
        target: Address::repeat_byte(18).to_string(),
        data: "0x1234".into(),
        value: "0".into(),
    }
}
fn signed_calls(wallet: &Wallet) -> (Vec<(Address, U256, Bytes)>, U256) {
    let sent = wallet.sent.lock().unwrap();
    let claim =
        super::claim::ClaimCoordinator::redeemAndExecuteCall::abi_decode(&sent[0].data).unwrap();
    let calls = claim
        .calls
        .into_iter()
        .map(|call| (call.target, call.value, call.callData))
        .collect();
    (calls, claim.minAmountOut)
}
fn approve_call(token: Address, spender: Address, amount: U256) -> (Address, U256, Bytes) {
    (
        token,
        U256::ZERO,
        crate::aa::abi::IERC20::approveCall { spender, amount }
            .abi_encode()
            .into(),
    )
}

#[tokio::test]
async fn outbound_cctp_claim_approves_the_whole_balance_and_accepts_a_burn_above_the_quote() {
    use crate::bridge::BridgeSubmission;
    use crate::bridge::OutboundBridge;
    use crate::bridge::OutboundClaim;
    use crate::bridge::OutboundReceipt;
    use crate::bridge::cctp;
    let (client, mut wallet, _store) = arbitrum_claim();
    let mut burn = crate::bridge::tests::burn();
    burn.source_chain_id = 42161;
    burn.source_domain = 3;
    burn.destination_domain = 6;
    burn.burn_token = cctp::CctpChain::for_chain(42161).unwrap().usdc;
    burn.sender = cctp::ARBITRUM_ADAPTER;
    let mut actual = burn.clone();
    actual.amount = U256::from(10_500_000);
    assert!(actual.amount > burn.amount);
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
        dex_calls: vec![outbound_dex_call()],
        bridge: OutboundBridge::Cctp {
            burn: burn.clone(),
            minimum_amount: U256::from(8_900_000),
        },
    };
    let result = client
        .claim_evm_to_bridge("test", &wallet, options, request, BridgeSubmission::Wallet)
        .await
        .unwrap();
    let OutboundReceipt::Cctp { burn: observed, .. } = result else {
        panic!("expected CCTP transfer")
    };
    assert_eq!(*observed, actual);
    let (calls, min_amount_out) = signed_calls(&wallet);
    assert_eq!(min_amount_out, U256::ZERO);
    assert_eq!(
        calls,
        vec![
            (
                Address::repeat_byte(18),
                U256::ZERO,
                Bytes::from(vec![0x12, 0x34])
            ),
            approve_call(burn.burn_token, cctp::ARBITRUM_ADAPTER, U256::MAX),
            (
                cctp::ARBITRUM_ADAPTER,
                U256::ZERO,
                cctp::bridgeBalanceCall {
                    destinationDomain: burn.destination_domain,
                    mintRecipient: burn.mint_recipient,
                    maxFee: burn.max_fee,
                    hookData: burn.hook.clone(),
                }
                .abi_encode()
                .into(),
            ),
            approve_call(burn.burn_token, cctp::ARBITRUM_ADAPTER, U256::ZERO),
        ]
    );
}

#[tokio::test]
async fn outbound_usdt0_claim_approves_the_whole_balance_and_accepts_a_send_above_the_quote() {
    use crate::bridge::BridgeSubmission;
    use crate::bridge::OutboundBridge;
    use crate::bridge::OutboundClaim;
    use crate::bridge::OutboundReceipt;
    use crate::bridge::usdt0;
    alloy::sol! {
        event BridgeInitiated(uint32 indexed dstEid, bytes32 to, uint256 amount, uint256 nativeFee, address indexed caller);
    }
    let (client, mut wallet, _store) = arbitrum_claim();
    let token = usdt0::CHAINS
        .iter()
        .find(|chain| chain.name == "Arbitrum")
        .unwrap()
        .token;
    let transfer = usdt0::OftTransfer {
        chain_id: 42161,
        source_eid: 30110,
        destination_eid: 30101,
        token,
        oft: usdt0::ARBITRUM_OFT,
        sender: usdt0::ARBITRUM_ADAPTER,
        recipient: B256::repeat_byte(77),
        amount: U256::from(10_000_000),
        minimum_received: U256::from(9_900_000),
        max_native_fee: U256::from(2000),
    };
    let bridged = U256::from(10_500_000);
    let guid = B256::repeat_byte(8);
    wallet.extra_logs.push(event_log(
        BridgeInitiated {
            dstEid: transfer.destination_eid,
            to: transfer.recipient,
            amount: bridged,
            nativeFee: U256::from(1200),
            caller: wallet.coordinator,
        },
        usdt0::ARBITRUM_ADAPTER,
    ));
    wallet.extra_logs.push(event_log(
        usdt0::OFTSent {
            guid,
            dstEid: transfer.destination_eid,
            fromAddress: usdt0::ARBITRUM_ADAPTER,
            amountSentLD: bridged,
            amountReceivedLD: bridged,
        },
        usdt0::ARBITRUM_OFT,
    ));
    let mut options = claim_options(&wallet);
    options.sweep_token = token;
    options.min_amount_out = U256::ZERO;
    let request = OutboundClaim {
        dex_calls: vec![outbound_dex_call()],
        bridge: OutboundBridge::Usdt0 {
            transfer: transfer.clone(),
        },
    };
    let result = client
        .claim_evm_to_bridge("test", &wallet, options, request, BridgeSubmission::Wallet)
        .await
        .unwrap();
    let OutboundReceipt::Usdt0(query) = result else {
        panic!("expected USDT0 transfer")
    };
    assert_eq!(query.guid, guid);
    let (calls, min_amount_out) = signed_calls(&wallet);
    assert_eq!(min_amount_out, U256::ZERO);
    assert_eq!(
        calls,
        vec![
            (
                Address::repeat_byte(18),
                U256::ZERO,
                Bytes::from(vec![0x12, 0x34])
            ),
            approve_call(token, usdt0::ARBITRUM_ADAPTER, U256::MAX),
            (
                usdt0::ARBITRUM_ADAPTER,
                U256::from(1200),
                usdt0::bridgeBalanceCall {
                    dstEid: transfer.destination_eid,
                    to: transfer.recipient,
                }
                .abi_encode()
                .into(),
            ),
            approve_call(token, usdt0::ARBITRUM_ADAPTER, U256::ZERO),
        ]
    );
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

#[tokio::test]
async fn a_different_recovery_does_not_need_the_settled_receipt() {
    let (client, wallet, _) = setup(false);
    let mut options = DepositRecovery {
        token: wallet.htlc.token,
        destination: Address::repeat_byte(80),
        amount: U256::from(100),
        timeout: Duration::from_secs(5),
    };
    client
        .recover_evm_deposit_with_wallet("test", &wallet, options.clone())
        .await
        .unwrap();
    // The mock reuses one transaction hash, so move the settled recovery onto a
    // hash whose receipt the node no longer serves.
    let old = B256::repeat_byte(55);
    client
        .update_swap_metadata(
            "test",
            SwapMetadata {
                evm_recovery_txid: Some(old.to_string()),
                evm_recovery_settled: Some(old.to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    wallet.missing_receipts.lock().unwrap().push(old);
    assert!(
        client
            .recover_evm_deposit_with_wallet("test", &wallet, options.clone())
            .await
            .is_err()
    );
    options.amount = U256::from(200);
    client
        .recover_evm_deposit_with_wallet("test", &wallet, options)
        .await
        .unwrap();
    assert_eq!(wallet.sent.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn second_recovery_lost_response_blocks_until_attached() {
    let (client, wallet, _) = setup(false);
    let mut options = DepositRecovery {
        token: wallet.htlc.token,
        destination: Address::repeat_byte(80),
        amount: U256::from(100),
        timeout: Duration::from_secs(5),
    };
    client
        .recover_evm_deposit_with_wallet("test", &wallet, options.clone())
        .await
        .unwrap();
    wallet.fail_send.store(true, Ordering::Relaxed);
    options.amount = U256::from(200);
    assert!(
        client
            .recover_evm_deposit_with_wallet("test", &wallet, options.clone())
            .await
            .is_err()
    );
    assert_eq!(wallet.sent.lock().unwrap().len(), 2);
    let err = client
        .recover_evm_deposit_with_wallet("test", &wallet, options.clone())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unresolved"), "{err}");
    assert_eq!(wallet.sent.lock().unwrap().len(), 2);
    client
        .attach_evm_recovery_transaction("test", B256::repeat_byte(77))
        .await
        .unwrap();
    client
        .recover_evm_deposit_with_wallet("test", &wallet, options)
        .await
        .unwrap();
    assert_eq!(wallet.sent.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn gasless_funding_rejects_unpinned_aa_deployments_before_any_payload_or_node_request() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let (_, _, fixture) = setup(false);
    let response = fixture.get_swap("test").unwrap().unwrap().response;
    let pinned_entry_point = crate::aa::userop::ENTRY_POINT_V07;
    let pinned_delegation = crate::aa::kernel::KERNEL_DELEGATION_TARGET;
    for (entry_point, delegation_target) in [
        (pinned_entry_point, Address::repeat_byte(0xde)),
        (Address::repeat_byte(0xee), pinned_delegation),
        (pinned_entry_point, pinned_delegation),
    ] {
        let store = Arc::new(InMemorySwapStorage::new());
        store
            .store_swap(StoredSwap::new(0, response.clone(), SwapMetadata::default()).unwrap())
            .unwrap();
        let server = MockServer::start().await;
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .build()
            .unwrap();
        Mock::given(method("GET"))
            .and(path("/swap/test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/aa/config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "entry_point": entry_point,
                "delegation_target": delegation_target,
                "permit2": crate::aa::permit2::PERMIT2_ADDRESS,
                "bundler": {"url": server.uri(), "casing": "camel"},
                "paymaster": {"url": server.uri()},
            })))
            .mount(&server)
            .await;
        let opts = crate::aa::GaslessOpts::new(server.uri().parse().unwrap());
        let result = client.fund_swap_gasless("test", opts).await;
        let paths: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| request.url.path().to_owned())
            .collect();
        if entry_point == pinned_entry_point && delegation_target == pinned_delegation {
            // Control: with the pinned deployment the flow reaches the payload fetch, so the
            // rejections above come from the pins and not from the fixture.
            assert!(paths.contains(&"/swap/test/swap-and-lock-calldata-userop".to_owned()));
            continue;
        }
        assert!(matches!(result, Err(Error::InvalidSwap(_))), "{result:?}");
        assert_eq!(paths, ["/swap/test", "/aa/config"]);
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
}

#[test]
fn derived_hash_lock_check() {
    let (client, _, store) = setup(false);
    let stored = store.get_swap("test").unwrap().unwrap();
    let context = EvmContext::from_response(&stored.response).unwrap();
    verify_derived_hash_lock(&client, &stored, &context).unwrap();
    let mut foreign = context.clone();
    foreign.hash = B256::repeat_byte(1);
    let rejected = verify_derived_hash_lock(&client, &stored, &foreign);
    assert!(
        matches!(&rejected, Err(Error::InvalidSwap(message)) if message.contains("seed-derived")),
        "{rejected:?}"
    );
    // Lightning-destination swaps lock under the payment hash of their stored invoice, which
    // pays 0x22..22 here, whatever hash the swap's hash lock names.
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let mut lightning = fixture["evm_to_lightning"].clone();
    lightning["evm_htlc_address"] = json!(context.htlc);
    lightning["server_evm_address"] = json!(context.server);
    lightning["hash_lock"] = json!(B256::repeat_byte(1));
    let lightning = StoredSwap::new(
        0,
        serde_json::from_value(lightning).unwrap(),
        SwapMetadata::default(),
    )
    .unwrap();
    let mut lightning_context = EvmContext::from_response(&lightning.response).unwrap();
    assert_eq!(lightning_context.hash, B256::repeat_byte(1));
    let rejected = verify_derived_hash_lock(&client, &lightning, &lightning_context);
    assert!(
        matches!(&rejected, Err(Error::InvalidSwap(message)) if message == crate::client::INVOICE_HASH_MISMATCH),
        "{rejected:?}"
    );
    lightning_context.hash = B256::repeat_byte(0x22);
    verify_derived_hash_lock(&client, &lightning, &lightning_context).unwrap();
    let unsigned = Client::new("http://localhost").unwrap();
    let rejected = verify_derived_hash_lock(&unsigned, &stored, &context);
    assert!(
        matches!(rejected, Err(Error::InvalidSigner(_))),
        "{rejected:?}"
    );
}

#[tokio::test]
async fn gasless_funding_rejects_foreign_hash_lock() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let (_, _, fixture) = setup(false);
    let response = fixture.get_swap("test").unwrap().unwrap().response;
    let store = Arc::new(InMemorySwapStorage::new());
    store
        .store_swap(StoredSwap::new(0, response.clone(), SwapMetadata::default()).unwrap())
        .unwrap();
    let mut served = serde_json::to_value(&response).unwrap();
    served["hash_lock"] = json!(B256::repeat_byte(1));
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/swap/test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&served))
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
    assert!(
        matches!(&result, Err(Error::InvalidSwap(message)) if message.contains("seed-derived")),
        "{result:?}"
    );
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
async fn lightning_funding_pays_only_a_lock_under_the_stored_invoices_payment_hash() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let (_, wallet, fixture) = setup_fixture("evm_to_lightning", false, EvmHtlcKind::Erc20);
    let response = fixture.get_swap("test").unwrap().unwrap().response;
    let store = Arc::new(InMemorySwapStorage::new());
    store
        .store_swap(StoredSwap::new(0, response.clone(), SwapMetadata::default()).unwrap())
        .unwrap();
    let options = FundingOptions {
        coordinator: wallet.coordinator,
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        max_source_amount: wallet.htlc.amount,
        allowed_call_targets: vec![],
        timeout: Duration::from_secs(10),
    };
    // Gasless funding locks under the hash a fresh server read names: one other than
    // the stored invoice's payment hash is refused before anything is signed.
    let mut served = serde_json::to_value(&response).unwrap();
    served["hash_lock"] = json!(B256::repeat_byte(1));
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/swap/test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&served))
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
    assert!(
        matches!(&result, Err(Error::InvalidSwap(message)) if message == crate::client::INVOICE_HASH_MISMATCH),
        "{result:?}"
    );
    let paths: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect();
    assert_eq!(paths, ["/swap/test"]);
    // Wallet funding locks under the stored hash lock, which an invoice stored before the
    // creation check may not pay.
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let mut inconsistent = serde_json::to_value(&response).unwrap();
    inconsistent["client_lightning_invoice"] =
        fixture["evm_to_lightning"]["client_lightning_invoice"].clone();
    store
        .store_swap(
            StoredSwap::new(
                0,
                serde_json::from_value(inconsistent).unwrap(),
                SwapMetadata::default(),
            )
            .unwrap(),
        )
        .unwrap();
    let result = client.fund_swap_with_wallet("test", &wallet, options).await;
    assert!(
        matches!(&result, Err(Error::InvalidSwap(message)) if message == crate::client::INVOICE_HASH_MISMATCH),
        "{result:?}"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(wallet.sent.lock().unwrap().is_empty());
    assert_eq!(wallet.signatures.load(Ordering::Relaxed), 0);
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
async fn refunds_read_the_servers_funding_hash_whatever_its_invoice() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::path;
    let (_, _, store) = setup_fixture("evm_to_lightning", false, EvmHtlcKind::Erc20);
    let vectors: Value =
        serde_json::from_str(include_str!("../../../fixtures/lightning-invoices.json")).unwrap();
    let mut served =
        serde_json::to_value(store.get_swap("test").unwrap().unwrap().response).unwrap();
    served["client_lightning_invoice"] = vectors["invoices"]["lnd_hold_33"]["bolt11"].clone();
    served["hash_lock"] = json!(B256::repeat_byte(0x33));
    served["evm_fund_txid"] = json!(B256::repeat_byte(7));
    let server = MockServer::start().await;
    Mock::given(path("/swap/test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(served))
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store)
        .build()
        .unwrap();
    assert!(client.get_swap_details("test").await.is_err());
    assert_eq!(
        client.server_funding_hash("test", "test.lookup").await,
        Some(B256::repeat_byte(7).to_string())
    );
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
        value.as_object_mut().unwrap().remove("evm_htlc_kind");
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

fn record_claim_destination(store: &InMemorySwapStorage, destination: Address) {
    rewrite_response(store, |r| r["target_evm_address"] = json!(destination));
    store
        .update_metadata(
            "test",
            SwapMetadata {
                target_address: Some(destination.to_string()),
                ..Default::default()
            },
        )
        .unwrap();
}
fn auto_claimer(
    client: Client,
    wallet: &Arc<Wallet>,
    deployments: Vec<crate::tracker::EvmDeployment>,
    user_op: Option<crate::aa::UserOpConfig>,
) -> crate::tracker::AutoClaimer {
    let mut claimer = crate::tracker::AutoClaimer::new(
        client,
        crate::tracker::AutoClaimConfig {
            evm: Some(crate::tracker::EvmAutoClaim {
                chains: [(
                    137,
                    crate::tracker::EvmAutoClaimChain {
                        rpc_url: "http://127.0.0.1:1".parse().unwrap(),
                        deployments,
                        user_op,
                    },
                )]
                .into(),
                ..Default::default()
            }),
            timeout: Duration::from_secs(10),
            ..Default::default()
        },
    )
    .unwrap();
    claimer.evm_wallet = Some(wallet.clone());
    claimer
}
fn deployment(wallet: &Wallet) -> crate::tracker::EvmDeployment {
    crate::tracker::EvmDeployment {
        htlc: wallet.htlc.address,
        coordinator: wallet.coordinator,
    }
}
async fn auto_claim(claimer: &crate::tracker::AutoClaimer) -> crate::tracker::ExecutionResult {
    use crate::tracker::ActionExecutor;
    claimer
        .execute("test", &crate::actions::ActionKind::Claim)
        .await
        .unwrap_or_else(|error| panic!("uncertain claim: {error}"))
}
/// The daemon's answer for the fixture's direct 100000 payout: its target less 1%,
/// with no calls.
async fn mount_calldata(server: &wiremock::MockServer, status: u16) {
    use alloy::primitives::keccak256;
    use alloy::sol_types::SolValue;
    let calls: Vec<crate::aa::abi::Call> = vec![];
    wiremock::Mock::given(wiremock::matchers::path("/swap/test/redeem-and-swap-calldata"))
        .respond_with(wiremock::ResponseTemplate::new(status).set_body_json(json!({"min_amount_out":"99000","gasless_fee_sats":0,"calls_hash":keccak256(calls.abi_encode()),"dex_calldata":null})))
        .mount(server)
        .await;
}
async fn mount_relay(server: &wiremock::MockServer, status: u16, expected: u64) {
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/swap/test/claim-gasless"))
        .respond_with(wiremock::ResponseTemplate::new(status).set_body_json(
            json!({"id":"test","tx_hash":B256::repeat_byte(9),"message":"submitted"}),
        ))
        .expect(expected)
        .mount(server)
        .await;
}

#[tokio::test]
async fn auto_claim_refuses_unlisted_contracts_other_destinations_and_bridged_payouts() {
    use crate::tracker::ExecutionResult::NotSent;
    let (_, wallet, store) = setup(true);
    let wallet = Arc::new(wallet);
    let server = wiremock::MockServer::start().await;
    mount_calldata(&server, 200).await;
    mount_relay(&server, 200, 0).await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    record_claim_destination(&store, wallet.owner);
    let listed = deployment(&wallet);
    for unlisted in [
        crate::tracker::EvmDeployment {
            coordinator: Address::repeat_byte(44),
            ..listed
        },
        crate::tracker::EvmDeployment {
            htlc: Address::repeat_byte(55),
            ..listed
        },
    ] {
        let claimer = auto_claimer(client.clone(), &wallet, vec![unlisted], None);
        let NotSent(error) = auto_claim(&claimer).await else {
            panic!("an unlisted deployment is refused before anything is sent");
        };
        assert!(error.to_string().contains("allowed deployment"), "{error}");
    }
    let claimer = auto_claimer(client.clone(), &wallet, vec![listed], None);
    rewrite_response(&store, |r| {
        r["target_evm_address"] = json!(Address::repeat_byte(66))
    });
    let NotSent(error) = auto_claim(&claimer).await else {
        panic!("a server destination other than the recorded one is refused");
    };
    assert!(error.to_string().contains("destination"), "{error}");
    record_claim_destination(&store, wallet.owner);
    rewrite_response(&store, |r| {
        r["bridge_target_chain"] = json!("Solana");
        r["bridge_target_token_address"] = json!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    });
    let NotSent(error) = auto_claim(&claimer).await else {
        panic!("a bridged payout is refused");
    };
    assert!(error.to_string().contains("bridge"), "{error}");
    assert_eq!(wallet.signatures.load(Ordering::Relaxed), 0);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn auto_claim_retries_a_relay_claim_only_while_nothing_was_sent() {
    use crate::tracker::ActionExecutor;
    use crate::tracker::ExecutionResult;
    let (_, wallet, store) = setup(true);
    let wallet = Arc::new(wallet);
    record_claim_destination(&store, wallet.owner);
    let client = |server: &wiremock::MockServer| {
        Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .build()
            .unwrap()
    };
    let unavailable = wiremock::MockServer::start().await;
    mount_calldata(&unavailable, 500).await;
    mount_relay(&unavailable, 200, 0).await;
    let claimer = auto_claimer(
        client(&unavailable),
        &wallet,
        vec![deployment(&wallet)],
        None,
    );
    assert!(matches!(
        auto_claim(&claimer).await,
        ExecutionResult::NotSent(_)
    ));
    let failing = wiremock::MockServer::start().await;
    mount_calldata(&failing, 200).await;
    mount_relay(&failing, 500, 1).await;
    let claimer = auto_claimer(client(&failing), &wallet, vec![deployment(&wallet)], None);
    assert!(
        claimer
            .execute("test", &crate::actions::ActionKind::Claim)
            .await
            .is_err()
    );
    let metadata = store.get_swap("test").unwrap().unwrap().metadata;
    assert_eq!(metadata.evm_claim_started, Some(true));
    // A recorded reservation is never run again.
    assert!(
        claimer
            .execute("test", &crate::actions::ActionKind::Claim)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn auto_claim_falls_back_to_the_relay_when_the_sponsored_claim_sent_nothing() {
    let (_, wallet, store) = setup(true);
    let wallet = Arc::new(wallet);
    record_claim_destination(&store, wallet.owner);
    let server = wiremock::MockServer::start().await;
    mount_calldata(&server, 200).await;
    mount_relay(&server, 200, 1).await;
    let node = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(500))
        .mount(&node)
        .await;
    let user_op = crate::aa::UserOpConfig {
        entry_point: Address::repeat_byte(12),
        delegation_target: Address::repeat_byte(13),
        aa: crate::aa::AaConfig {
            node_rpc_url: node.uri().parse().unwrap(),
            bundler_url: node.uri().parse().unwrap(),
            paymaster: None,
            bundler_casing: Default::default(),
            gas_overrides: None,
        },
    };
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let claimer = auto_claimer(client, &wallet, vec![deployment(&wallet)], Some(user_op));
    let crate::tracker::ExecutionResult::Submitted { reference } = auto_claim(&claimer).await
    else {
        panic!("the relay claim is submitted");
    };
    assert_eq!(reference, B256::repeat_byte(9).to_string());
    assert!(!node.received_requests().await.unwrap().is_empty());
    let metadata = store.get_swap("test").unwrap().unwrap().metadata;
    assert_eq!(metadata.evm_claim_txid, Some(reference));
    assert_eq!(metadata.evm_claim_userop_hash, None);
}

/// Auto-claims a `fixture` swap with target 100000 through a relay whose calldata
/// answers with `minimum` and `calls_hash`, expecting `relayed` claim requests.
async fn relay_auto_claim(
    fixture: &str,
    dex: bool,
    minimum: u64,
    calls_hash: B256,
    relayed: bool,
) -> (crate::tracker::ExecutionResult, Arc<Wallet>) {
    let (_, wallet, store) = setup_fixture(fixture, true, EvmHtlcKind::Erc20);
    let wallet = Arc::new(wallet);
    record_claim_destination(&store, wallet.owner);
    rewrite_response(&store, |r| {
        r["target_amount"] = json!("100000");
        if dex {
            r["target_token"]["token_id"] = json!(Address::repeat_byte(77));
        }
    });
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/swap/test/redeem-and-swap-calldata"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({"min_amount_out":minimum.to_string(),"gasless_fee_sats":0,"calls_hash":calls_hash,"dex_calldata":null})))
        .mount(&server)
        .await;
    mount_relay(&server, 200, u64::from(relayed)).await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let claimer = auto_claimer(client, &wallet, vec![deployment(&wallet)], None);
    (auto_claim(&claimer).await, wallet)
}

#[tokio::test]
async fn auto_claim_relays_a_payout_at_the_daemon_slippage_floor_below_its_target() {
    use crate::tracker::ExecutionResult;
    use alloy::primitives::keccak256;
    use alloy::sol_types::SolValue;
    let no_calls = keccak256(Vec::<crate::aa::abi::Call>::new().abi_encode());
    for (fixture, floor) in [("arkade_to_evm", 99_000), ("bitcoin_to_evm", 98_000)] {
        for dex in [false, true] {
            let calls_hash = if dex { B256::repeat_byte(1) } else { no_calls };
            for (minimum, submitted) in [(floor - 1, false), (floor, true)] {
                let (result, _) =
                    relay_auto_claim(fixture, dex, minimum, calls_hash, submitted).await;
                let case = format!("{fixture}, dex {dex}, minimum {minimum}");
                if submitted {
                    assert!(
                        matches!(result, ExecutionResult::Submitted { .. }),
                        "{case}"
                    );
                } else {
                    let ExecutionResult::NotSent(error) = result else {
                        panic!("{case}: a minimum below the floor is refused");
                    };
                    assert!(error.to_string().contains("minimum"), "{case}: {error}");
                }
            }
        }
    }
}

#[tokio::test]
async fn auto_claim_refuses_server_calls_on_a_direct_relay_payout() {
    let (result, wallet) =
        relay_auto_claim("arkade_to_evm", false, 99_000, B256::repeat_byte(1), false).await;
    let crate::tracker::ExecutionResult::NotSent(error) = result else {
        panic!("calls that could divert part of the locked token are refused");
    };
    assert!(error.to_string().contains("server calls"), "{error}");
    assert_eq!(wallet.signatures.load(Ordering::Relaxed), 0);
}

fn relay_source() -> Address {
    Address::repeat_byte(0x11)
}
fn relay_dex() -> Address {
    Address::repeat_byte(0x0d)
}
fn relay_domain() -> B256 {
    B256::repeat_byte(0xd0)
}
fn relay_calls(fee: u64, spender: Address) -> Vec<crate::aa::abi::Call> {
    use crate::aa::abi::Call;
    let mut calls = vec![];
    if fee > 0 {
        calls.push(Call {
            target: relay_source(),
            value: U256::ZERO,
            callData: super::recovery::RecoveryToken::transferCall {
                to: Address::repeat_byte(3),
                amount: U256::from(fee),
            }
            .abi_encode()
            .into(),
        });
    }
    calls.push(Call {
        target: relay_source(),
        value: U256::ZERO,
        callData: crate::aa::abi::IERC20::approveCall {
            spender,
            amount: U256::from(100_000 - fee),
        }
        .abi_encode()
        .into(),
    });
    calls.push(Call {
        target: relay_dex(),
        value: U256::ZERO,
        callData: Bytes::from_static(&[0x12, 0x34]),
    });
    calls
}
fn set_relay_calls(payload: &mut Value, calls: &[crate::aa::abi::Call]) {
    use alloy::sol_types::SolValue;
    payload["calls"] = calls
        .iter()
        .map(|c| json!({"target": c.target, "value": c.value.to_string(), "call_data": c.callData}))
        .collect();
    payload["calls_hash"] = json!(alloy::primitives::keccak256(calls.abi_encode()));
}
fn relay_payload(wallet: &Wallet) -> Value {
    let mut payload = json!({
        "coordinator_address": wallet.coordinator,
        "permit2_address": crate::aa::permit2::PERMIT2_ADDRESS,
        "source_token_address": relay_source(),
        "source_amount": "100000",
        "lock_token_address": wallet.htlc.token,
        "preimage_hash": wallet.htlc.hash_lock,
        "claim_address": wallet.htlc.claimer,
        "timelock": wallet.htlc.timelock,
        "eip2612": {"supported": true, "already_approved": false, "nonce": 0, "domain_separator": relay_domain()},
        "relay_fee": "300",
        "depositor_delegation": null,
    });
    set_relay_calls(&mut payload, &relay_calls(300, relay_dex()));
    payload
}
fn relay_pinned(max_source_amount: u64, allowed_call_targets: Vec<Address>) -> FundingOptions {
    FundingOptions {
        coordinator: Address::repeat_byte(4),
        htlc: Address::repeat_byte(5),
        lock_token: Address::repeat_byte(6),
        max_source_amount: U256::from(max_source_amount),
        allowed_call_targets,
        timeout: Duration::from_secs(10),
    }
}
fn relay_deadline() -> tokio::time::Instant {
    tokio::time::Instant::now() + Duration::from_secs(10)
}

type SeenJournal = (
    Option<crate::storage::RelayFundingOutcome>,
    Option<String>,
    Option<bool>,
);

/// Stands in for the daemon's relay: records each request with the journal it found,
/// and on success spends the posted Permit2 nonce as the funding transaction would.
#[derive(Clone)]
struct RelayServer {
    store: Arc<InMemorySwapStorage>,
    spent: Arc<Mutex<Vec<U256>>>,
    bodies: Arc<Mutex<Vec<Value>>>,
    seen: Arc<Mutex<Vec<SeenJournal>>>,
    statuses: Arc<Mutex<Vec<u16>>>,
    /// Shared, so that it can change after the responder is mounted.
    delay: Arc<Mutex<Duration>>,
    /// The body of a 200.
    answer: Value,
}
impl wiremock::Respond for RelayServer {
    fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let metadata = self.store.get_swap("test").unwrap().unwrap().metadata;
        let journal = metadata.evm_fund_relay;
        self.seen.lock().unwrap().push((
            journal.as_ref().map(|journal| journal.outcome),
            journal.map(|journal| journal.permit2_nonce),
            metadata.evm_fund_started,
        ));
        self.bodies.lock().unwrap().push(body.clone());
        let status = {
            let mut statuses = self.statuses.lock().unwrap();
            if statuses.is_empty() {
                200
            } else {
                statuses.remove(0)
            }
        };
        let response = if status == 200 {
            if let Some(nonce) = body["permit2_nonce"].as_str() {
                self.spent
                    .lock()
                    .unwrap()
                    .push(U256::from_str_radix(nonce, 10).unwrap());
            }
            wiremock::ResponseTemplate::new(200).set_body_json(&self.answer)
        } else {
            wiremock::ResponseTemplate::new(status).set_body_json(json!({"error": "refused"}))
        };
        response.set_delay(*self.delay.lock().unwrap())
    }
}

struct RelayCase {
    client: Client,
    wallet: Wallet,
    store: Arc<InMemorySwapStorage>,
    server: wiremock::MockServer,
    relay: RelayServer,
    payload: Value,
}
impl RelayCase {
    async fn new(
        edit_response: impl FnOnce(&mut Value),
        edit_payload: impl FnOnce(&mut Value),
    ) -> Self {
        Self::for_direction("evm_to_arkade", edit_response, edit_payload).await
    }
    async fn for_direction(
        direction: &str,
        edit_response: impl FnOnce(&mut Value),
        edit_payload: impl FnOnce(&mut Value),
    ) -> Self {
        use wiremock::Mock;
        use wiremock::ResponseTemplate;
        use wiremock::matchers::method;
        use wiremock::matchers::path;
        let (_, wallet, fixture) = setup_direction(direction, EvmHtlcKind::Erc20);
        *wallet.permit_domain.lock().unwrap() = relay_domain();
        let mut response =
            serde_json::to_value(fixture.get_swap("test").unwrap().unwrap().response).unwrap();
        response["gasless"] = json!(true);
        response["source_token"]["token_id"] = json!(relay_source());
        edit_response(&mut response);
        let mut payload = relay_payload(&wallet);
        edit_payload(&mut payload);
        let store = Arc::new(InMemorySwapStorage::new());
        store
            .store_swap(
                StoredSwap::new(
                    0,
                    serde_json::from_value(response.clone()).unwrap(),
                    SwapMetadata {
                        evm_coordinator_address: Some(wallet.coordinator.to_string()),
                        ..Default::default()
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let server = wiremock::MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/swap/test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/swap/test/swap-and-lock-calldata-permit2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .build()
            .unwrap();
        let relay = RelayServer {
            store: store.clone(),
            spent: wallet.spent_nonces.clone(),
            bodies: Arc::default(),
            seen: Arc::default(),
            statuses: Arc::default(),
            delay: Arc::default(),
            answer: json!({"id": "test", "tx_hash": B256::repeat_byte(42), "message": "funded"}),
        };
        Self {
            client,
            wallet,
            store,
            server,
            relay,
            payload,
        }
    }
    async fn mount_relay(&self, expected: u64) {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/swap/test/fund-gasless"))
            .respond_with(self.relay.clone())
            .expect(expected)
            .mount(&self.server)
            .await;
    }
    async fn fund(&self, options: &RelayFundingOptions) -> crate::Result<RelayFunding> {
        self.client
            .fund_relay("test", Some(&self.wallet), options, relay_deadline())
            .await
    }
    /// Through the operation timeout, as `fund_swap_gasless_relay` runs it.
    async fn fund_within(&self, options: &RelayFundingOptions) -> crate::Result<RelayFunding> {
        self.client
            .fund_relay_within("test", Some(&self.wallet), options)
            .await
    }
    /// Asserts a refusal before anything was reserved or posted.
    async fn assert_refused(&self, options: &RelayFundingOptions, expected: &str, name: &str) {
        self.mount_relay(0).await;
        let result = self.fund(options).await;
        assert!(
            matches!(&result, Err(Error::InvalidSwap(message)) if message.contains(expected)),
            "{name}: {result:?}"
        );
        let metadata = self.metadata();
        assert_eq!(metadata.evm_fund_relay, None, "{name}");
        assert_eq!(metadata.evm_fund_started, None, "{name}");
        self.server.verify().await;
    }
    /// Serves `GET /swap/test` from an edited copy of the stored response.
    async fn serve_swap(&self, edit: impl FnOnce(&mut Value)) {
        let mut response =
            serde_json::to_value(self.store.get_swap("test").unwrap().unwrap().response).unwrap();
        edit(&mut response);
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/swap/test"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(&response))
            .with_priority(1)
            .mount(&self.server)
            .await;
    }
    async fn reconcile(&self) -> crate::Result<RelayFundingState> {
        self.client
            .reconcile_relay(
                "test",
                Some(&self.wallet),
                &RelayFundingOptions::new(U256::ZERO),
            )
            .await
    }
    fn metadata(&self) -> SwapMetadata {
        self.store.get_swap("test").unwrap().unwrap().metadata
    }
    fn journal(&self) -> crate::storage::RelayFundingJournal {
        self.metadata().evm_fund_relay.unwrap()
    }
    fn body(&self, index: usize) -> Value {
        self.relay.bodies.lock().unwrap()[index].clone()
    }
    fn permit2_digest(&self, body: &Value) -> B256 {
        let funding: Permit2FundingData = serde_json::from_value(self.payload.clone()).unwrap();
        funding
            .typed_data(
                137,
                U256::from_str_radix(body["permit2_nonce"].as_str().unwrap(), 10).unwrap(),
                U256::from(body["permit2_deadline"].as_u64().unwrap()),
            )
            .unwrap()
            .digest
    }
    fn permit_digest(&self, body: &Value) -> B256 {
        super::permit::permit_digest(
            relay_domain(),
            self.wallet.owner,
            crate::aa::permit2::PERMIT2_ADDRESS,
            U256::MAX,
            U256::ZERO,
            U256::from(body["eip2612_permit"]["deadline"].as_u64().unwrap()),
        )
    }
}
fn signature_bytes(value: &Value) -> Bytes {
    value.as_str().unwrap().parse().unwrap()
}
fn recovers(signature: &[u8], digest: B256) -> Address {
    Signature::try_from(signature)
        .unwrap()
        .recover_address_from_prehash(&digest)
        .unwrap()
}

#[tokio::test]
async fn relay_rejects_substituted_payload_without_posting() {
    type Case = (
        &'static str,
        &'static str,
        fn(&mut Value),
        fn(&mut Value),
        fn(&mut RelayFundingOptions),
    );
    fn keep(_: &mut Value) {}
    fn defaults(_: &mut RelayFundingOptions) {}
    let control = RelayCase::new(keep, keep).await;
    control.mount_relay(1).await;
    control
        .fund(&RelayFundingOptions::new(U256::from(300)))
        .await
        .unwrap();
    let cases: Vec<Case> = vec![
        (
            "coordinator",
            "does not match the swap",
            keep,
            |p| p["coordinator_address"] = json!(Address::repeat_byte(0x44)),
            defaults,
        ),
        (
            "hash lock",
            "does not match the swap",
            keep,
            |p| p["preimage_hash"] = json!(B256::repeat_byte(1)),
            defaults,
        ),
        (
            "claim address",
            "does not match the swap",
            keep,
            |p| p["claim_address"] = json!(Address::repeat_byte(0x33)),
            defaults,
        ),
        (
            "Permit2 address",
            "does not match the swap",
            keep,
            |p| p["permit2_address"] = json!(Address::repeat_byte(0x22)),
            defaults,
        ),
        (
            "calls hash",
            "calls hash mismatch",
            keep,
            |p| p["calls_hash"] = json!(B256::ZERO),
            defaults,
        ),
        (
            "amount over the stored swap",
            "exceeds the approved limit",
            keep,
            |p| p["source_amount"] = json!("100001"),
            defaults,
        ),
        (
            "amount over the pinned cap",
            "exceeds the approved limit",
            keep,
            keep,
            |o| {
                o.funding = Some(relay_pinned(99_999, vec![relay_dex()]));
            },
        ),
        (
            "fee to another address",
            "relay fee",
            keep,
            |p| {
                let mut calls = relay_calls(300, relay_dex());
                calls[0].callData = super::recovery::RecoveryToken::transferCall {
                    to: Address::repeat_byte(0x77),
                    amount: U256::from(300),
                }
                .abi_encode()
                .into();
                set_relay_calls(p, &calls);
            },
            defaults,
        ),
        ("fee over the maximum", "relay fee", keep, keep, |o| {
            o.max_relay_fee = U256::from(299)
        }),
        (
            "fee differs from the quote",
            "relay fee",
            keep,
            |p| p["relay_fee"] = json!("200"),
            defaults,
        ),
        (
            "unlisted approve spender",
            "unapproved spender",
            keep,
            |p| {
                set_relay_calls(p, &relay_calls(300, Address::repeat_byte(0x66)));
            },
            |o| o.funding = Some(relay_pinned(100_000, vec![relay_dex()])),
        ),
        (
            "unlisted call target",
            "unapproved call target",
            keep,
            |p| {
                set_relay_calls(p, &relay_calls(300, Address::repeat_byte(0x66)));
            },
            |o| o.funding = Some(relay_pinned(100_000, vec![Address::repeat_byte(0x66)])),
        ),
        (
            "approval above the input",
            "more than the input",
            keep,
            |p| {
                let mut calls = relay_calls(300, relay_dex());
                calls[1].callData = crate::aa::abi::IERC20::approveCall {
                    spender: relay_dex(),
                    amount: U256::from(100_001),
                }
                .abi_encode()
                .into();
                set_relay_calls(p, &calls);
            },
            defaults,
        ),
        (
            "source-token transfer after the fee",
            "more than the fee and an approval",
            keep,
            |p| {
                let mut calls = relay_calls(300, relay_dex());
                calls.swap(0, 1);
                set_relay_calls(p, &calls);
            },
            defaults,
        ),
        (
            "native value in a call",
            "native-token transfer",
            keep,
            |p| {
                let mut calls = relay_calls(300, relay_dex());
                calls[2].value = U256::ONE;
                set_relay_calls(p, &calls);
            },
            defaults,
        ),
        (
            "swap not gasless",
            "created as gasless",
            |r| r["gasless"] = json!(false),
            keep,
            defaults,
        ),
        (
            "native lock",
            "native lock",
            |r| r["evm_htlc_kind"] = json!(EvmHtlcKind::Native),
            keep,
            defaults,
        ),
        (
            "server domain separator",
            "differs from the chain",
            keep,
            |p| {
                p["eip2612"]["domain_separator"] = json!(B256::repeat_byte(0xee));
            },
            defaults,
        ),
        (
            "server permit nonce",
            "differs from the chain",
            keep,
            |p| p["eip2612"]["nonce"] = json!(7),
            defaults,
        ),
        (
            "no allowance and no permit support",
            "no EIP-2612 permit",
            keep,
            |p| {
                p["eip2612"]["supported"] = json!(false);
            },
            defaults,
        ),
        (
            "unknown delegation target",
            "cannot sign for",
            keep,
            |p| {
                p["depositor_delegation"] = json!(Address::repeat_byte(0x99));
            },
            defaults,
        ),
        (
            "signatures outliving the timelock",
            "past the refund timelock",
            |r| {
                r["evm_refund_locktime"] = json!(super::funding::now().unwrap() + 600);
            },
            |p| p["timelock"] = json!(super::funding::now().unwrap() + 600),
            defaults,
        ),
    ];
    for (name, expected, edit_response, edit_payload, edit_options) in cases {
        let case = RelayCase::new(edit_response, edit_payload).await;
        let mut options = RelayFundingOptions::new(U256::from(300));
        edit_options(&mut options);
        case.assert_refused(&options, expected, name).await;
    }

    let pinned = RelayCase::new(keep, keep).await;
    pinned.mount_relay(1).await;
    let mut options = RelayFundingOptions::new(U256::from(300));
    options.funding = Some(relay_pinned(100_000, vec![relay_dex()]));
    pinned.fund(&options).await.unwrap();

    let unhinted: fn(&mut Value) = |p| {
        p.as_object_mut().unwrap().remove("depositor_delegation");
    };
    type ChainCase = (&'static str, &'static str, fn(&mut Value), fn(&Wallet));
    let chain_cases: Vec<ChainCase> = vec![
        (
            "insufficient balance",
            "insufficient token balance",
            keep,
            |w| w.token_balance.store(99_999, Ordering::Relaxed),
        ),
        (
            "probed code that is not a delegation",
            "not a delegation",
            unhinted,
            |w| *w.code.lock().unwrap() = Some(Bytes::from_static(&[0x60, 0x80, 0x60, 0x40])),
        ),
        (
            "probed delegation to unknown code",
            "cannot sign for",
            unhinted,
            |w| {
                let mut code = vec![0xef, 0x01, 0x00];
                code.extend_from_slice(Address::repeat_byte(0x99).as_slice());
                *w.code.lock().unwrap() = Some(code.into());
            },
        ),
    ];
    for (name, expected, edit_payload, edit_wallet) in chain_cases {
        let case = RelayCase::new(keep, edit_payload).await;
        edit_wallet(&case.wallet);
        case.assert_refused(&RelayFundingOptions::new(U256::from(300)), expected, name)
            .await;
    }
}

#[tokio::test]
async fn relay_checks_the_server_and_persisted_copies_of_the_swap() {
    let options = RelayFundingOptions::new(U256::from(300));
    let coordinator: fn(&mut Value) =
        |r| r["evm_coordinator_address"] = json!(Address::repeat_byte(0x44));
    let htlc: fn(&mut Value) = |r| r["evm_htlc_address"] = json!(Address::repeat_byte(0x55));
    for (name, edit) in [("served coordinator", coordinator), ("served HTLC", htlc)] {
        let case = RelayCase::new(|_| {}, |_| {}).await;
        case.serve_swap(edit).await;
        case.assert_refused(&options, "differ from the stored swap", name)
            .await;
    }
    let case = RelayCase::new(|_| {}, |_| {}).await;
    case.client
        .update_swap_metadata(
            "test",
            SwapMetadata {
                evm_coordinator_address: Some(Address::repeat_byte(0x44).to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    case.assert_refused(&options, "persisted for this swap", "persisted coordinator")
        .await;
}

#[tokio::test]
async fn relay_binds_the_stored_lock_token() {
    let other = Address::repeat_byte(0x77);
    let substituted = move |p: &mut Value| p["lock_token_address"] = json!(other);
    let options = RelayFundingOptions::new(U256::from(300));
    for direction in ["evm_to_bitcoin", "evm_to_lightning"] {
        let control = RelayCase::for_direction(direction, |_| {}, |_| {}).await;
        control.mount_relay(1).await;
        control.fund(&options).await.unwrap();
        assert_eq!(
            control.journal().lock_token,
            control.wallet.htlc.token.to_string()
        );

        let case = RelayCase::for_direction(direction, |_| {}, substituted).await;
        let name = format!("{direction}: substituted payload token");
        case.assert_refused(&options, "does not match the swap", &name)
            .await;

        // A pinned token cannot stand in for the stored one, even when the payload agrees.
        let case = RelayCase::for_direction(direction, |_| {}, substituted).await;
        let mut pinned = relay_pinned(100_000, vec![relay_dex()]);
        pinned.lock_token = other;
        let mut pinned_options = options.clone();
        pinned_options.funding = Some(pinned);
        let name = format!("{direction}: pinned token");
        case.assert_refused(&pinned_options, "does not match the swap", &name)
            .await;

        let case = RelayCase::for_direction(direction, |_| {}, |_| {}).await;
        case.serve_swap(|r| r["wbtc_address"] = json!(other)).await;
        let name = format!("{direction}: served token");
        case.assert_refused(&options, "differ from the stored swap", &name)
            .await;
    }
}

#[tokio::test]
async fn relay_persists_intent_before_posting_and_verifies_receipt() {
    use crate::storage::RelayFundingOutcome;
    for allowance in [0, 100_000] {
        let mut case = RelayCase::new(|_| {}, |_| {}).await;
        case.wallet
            .permit2_allowance
            .store(allowance, Ordering::Relaxed);
        // The relay answers only after its receipt, so the request must outlive the
        // client-wide timeout.
        case.client = Client::builder()
            .base_url(case.server.uri())
            .mnemonic(MNEMONIC)
            .storage(case.store.clone())
            .http(
                reqwest::Client::builder()
                    .timeout(Duration::from_secs(1))
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        *case.relay.delay.lock().unwrap() = Duration::from_millis(1500);
        case.mount_relay(1).await;
        let funded = case
            .fund(&RelayFundingOptions::new(U256::from(300)))
            .await
            .unwrap();
        let body = case.body(0);
        let nonce = body["permit2_nonce"].as_str().unwrap().to_owned();
        assert_eq!(
            case.relay.seen.lock().unwrap()[0],
            (Some(RelayFundingOutcome::Sending), Some(nonce), Some(true))
        );
        assert_eq!(body["calls"], case.payload["calls"]);
        let signature = signature_bytes(&body["permit2_signature"]);
        assert_eq!(signature.len(), 65);
        assert_eq!(
            recovers(&signature, case.permit2_digest(&body)),
            case.wallet.owner
        );
        if allowance == 0 {
            let permit = &body["eip2612_permit"];
            assert_eq!(permit["value"], json!(U256::MAX.to_string()));
            assert_eq!(permit["deadline"], body["permit2_deadline"]);
            let parse = |field: &str| {
                U256::from_str_radix(permit[field].as_str().unwrap().trim_start_matches("0x"), 16)
                    .unwrap()
            };
            let signature = Signature::new(parse("r"), parse("s"), permit["v"] == json!(28));
            assert_eq!(
                signature
                    .recover_address_from_prehash(&case.permit_digest(&body))
                    .unwrap(),
                case.wallet.owner
            );
        } else {
            assert!(body.get("eip2612_permit").is_none());
        }
        assert_eq!(funded.tx_hash, B256::repeat_byte(42));
        assert_eq!(funded.htlc, case.wallet.htlc);
        assert_eq!(funded.eip2612_permit, allowance == 0);
        let metadata = case.metadata();
        assert_eq!(
            metadata.evm_fund_txid,
            Some(B256::repeat_byte(42).to_string())
        );
        let journal = metadata.evm_fund_relay.unwrap();
        assert_eq!(journal.outcome, RelayFundingOutcome::Funded);
        assert_eq!(journal.tx_hash, metadata.evm_fund_txid);
    }
}

#[tokio::test]
async fn relay_lost_response_is_reconciled_not_resent() {
    use crate::storage::RelayFundingOutcome;
    // A 502, then a relay that holds the request past the operation deadline. Its
    // late 400 would count as a refusal, so only the deadline leaves it uncertain.
    for (status, delay, budget) in [
        (502, Duration::ZERO, Duration::from_secs(10)),
        (400, Duration::from_secs(20), Duration::from_secs(3)),
    ] {
        let case = RelayCase::new(|_| {}, |_| {}).await;
        case.relay.statuses.lock().unwrap().push(status);
        *case.relay.delay.lock().unwrap() = delay;
        case.mount_relay(2).await;
        let options = RelayFundingOptions::new(U256::from(300));
        let lost = case
            .client
            .fund_relay(
                "test",
                Some(&case.wallet),
                &options,
                tokio::time::Instant::now() + budget,
            )
            .await;
        assert!(lost.is_err());
        *case.relay.delay.lock().unwrap() = Duration::ZERO;
        let journal = case.journal();
        assert_eq!(journal.outcome, RelayFundingOutcome::Uncertain);
        assert_eq!(case.metadata().evm_fund_started, Some(true));
        assert!(case.fund(&options).await.is_err());
        assert_eq!(case.relay.bodies.lock().unwrap().len(), 1);

        case.wallet
            .now
            .store(journal.permit2_deadline, Ordering::Relaxed);
        assert!(matches!(
            case.reconcile().await.unwrap(),
            RelayFundingState::Pending { nonce_consumed: false, deadline } if deadline == journal.permit2_deadline
        ));
        assert_eq!(case.metadata().evm_fund_started, Some(true));
        // A lagging node may still be behind a block that spends the nonce.
        case.wallet
            .now
            .store(journal.permit2_deadline + 120, Ordering::Relaxed);
        assert!(matches!(
            case.reconcile().await.unwrap(),
            RelayFundingState::Pending {
                nonce_consumed: false,
                ..
            }
        ));
        case.wallet
            .now
            .store(journal.permit2_deadline + 121, Ordering::Relaxed);
        assert!(matches!(
            case.reconcile().await.unwrap(),
            RelayFundingState::Expired
        ));
        let metadata = case.metadata();
        assert_eq!(metadata.evm_fund_started, Some(false));
        assert_eq!(
            metadata.evm_fund_relay.unwrap().outcome,
            RelayFundingOutcome::Expired
        );

        // Nothing can land any more, and a new attempt still signs the same nonce.
        case.fund(&options).await.unwrap();
        assert_eq!(case.body(0)["permit2_nonce"], case.body(1)["permit2_nonce"]);
    }
}

#[tokio::test]
async fn relay_lost_response_is_funded_from_the_server_hash() {
    use crate::storage::RelayFundingOutcome;
    use wiremock::Mock;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let case = RelayCase::new(|_| {}, |_| {}).await;
    case.relay.statuses.lock().unwrap().push(502);
    case.mount_relay(1).await;
    assert!(
        case.fund(&RelayFundingOptions::new(U256::from(300)))
            .await
            .is_err()
    );
    let journal = case.journal();
    // The relay funded the swap but its answer was lost.
    case.wallet
        .spent_nonces
        .lock()
        .unwrap()
        .push(U256::from_str_radix(&journal.permit2_nonce, 10).unwrap());
    case.wallet.now.store(u64::MAX / 2, Ordering::Relaxed);
    assert!(matches!(
        case.reconcile().await.unwrap(),
        RelayFundingState::Pending {
            nonce_consumed: true,
            ..
        }
    ));
    assert_eq!(case.metadata().evm_fund_started, Some(true));
    let mut response =
        serde_json::to_value(case.store.get_swap("test").unwrap().unwrap().response).unwrap();
    response["evm_fund_txid"] = json!(B256::repeat_byte(42));
    Mock::given(method("GET"))
        .and(path("/swap/test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&response))
        .with_priority(1)
        .mount(&case.server)
        .await;
    // A matching receipt alone is not enough while the nonce reads unspent.
    let spent = case.wallet.spent_nonces.lock().unwrap().split_off(0);
    case.wallet
        .now
        .store(journal.permit2_deadline, Ordering::Relaxed);
    assert!(matches!(
        case.reconcile().await.unwrap(),
        RelayFundingState::Pending {
            nonce_consumed: false,
            ..
        }
    ));
    case.wallet.spent_nonces.lock().unwrap().extend(spent);
    let RelayFundingState::Funded(funded) = case.reconcile().await.unwrap() else {
        panic!("expected the server's hash to prove the funding")
    };
    assert_eq!(funded.tx_hash, B256::repeat_byte(42));
    let metadata = case.metadata();
    assert_eq!(
        metadata.evm_fund_txid,
        Some(B256::repeat_byte(42).to_string())
    );
    assert_eq!(
        metadata.evm_fund_relay.unwrap().outcome,
        RelayFundingOutcome::Funded
    );
}

#[tokio::test]
async fn relay_rejection_resigns_with_the_same_nonce() {
    use crate::storage::RelayFundingOutcome;
    let case = RelayCase::new(|_| {}, |_| {}).await;
    case.relay.statuses.lock().unwrap().push(400);
    case.mount_relay(2).await;
    let rejected = case.fund(&RelayFundingOptions::new(U256::from(300))).await;
    assert!(
        matches!(rejected, Err(Error::Api { status: 400, .. })),
        "{rejected:?}"
    );
    assert_eq!(case.journal().outcome, RelayFundingOutcome::Rejected);
    // The refused signature stays valid until its deadline, so other funding paths wait.
    assert_eq!(case.metadata().evm_fund_started, Some(true));
    assert!(
        case.client
            .fund_swap_with_wallet("test", &case.wallet, relay_pinned(100_000, vec![]))
            .await
            .is_err()
    );
    let mut shorter = RelayFundingOptions::new(U256::from(300));
    shorter.permit_deadline = Duration::from_secs(600);
    case.fund(&shorter).await.unwrap();
    let (first, second) = (case.body(0), case.body(1));
    assert_eq!(first["permit2_nonce"], second["permit2_nonce"]);
    let first_deadline = first["permit2_deadline"].as_u64().unwrap();
    assert!(second["permit2_deadline"].as_u64().unwrap() < first_deadline);
    let seen = case.relay.seen.lock().unwrap().clone();
    assert_eq!(seen[1].0, Some(RelayFundingOutcome::Sending));
    let journal = case.journal();
    assert_eq!(journal.outcome, RelayFundingOutcome::Funded);
    assert_eq!(journal.permit2_deadline, first_deadline);
}

#[tokio::test]
async fn relay_unproven_answers_block_another_attempt() {
    use crate::storage::RelayFundingOutcome;
    let options = RelayFundingOptions::new(U256::from(300));
    fn reconcile_first(result: &crate::Result<RelayFunding>) -> bool {
        matches!(result, Err(Error::UnresolvedFunding(message)) if message.contains("reconcile"))
    }
    // The mock serves any other hash a receipt without this HTLC's creation.
    let unproven = B256::repeat_byte(43);
    for (answer, outcome, tx_hash) in [
        (
            json!({"id": "test", "tx_hash": unproven, "message": "funded"}),
            RelayFundingOutcome::Accepted,
            Some(unproven.to_string()),
        ),
        (
            json!({"id": "other", "tx_hash": B256::repeat_byte(42), "message": "funded"}),
            RelayFundingOutcome::Uncertain,
            None,
        ),
        (
            json!({"id": "test", "tx_hash": "queued", "message": "funded"}),
            RelayFundingOutcome::Uncertain,
            None,
        ),
    ] {
        let mut case = RelayCase::new(|_| {}, |_| {}).await;
        case.relay.answer = answer;
        case.mount_relay(1).await;
        assert!(case.fund(&options).await.is_err());
        let metadata = case.metadata();
        assert_eq!(metadata.evm_fund_txid, None);
        let journal = metadata.evm_fund_relay.unwrap();
        assert_eq!((journal.outcome, journal.tx_hash), (outcome, tx_hash));
        let again = case.fund(&options).await;
        assert!(reconcile_first(&again), "{outcome:?}: {again:?}");
        case.server.verify().await;
    }

    // A process that stopped while sending leaves the journal at Sending.
    let case = RelayCase::new(|_| {}, |_| {}).await;
    case.client
        .update_swap_metadata(
            "test",
            SwapMetadata {
                evm_fund_relay: Some(crate::storage::RelayFundingJournal {
                    permit2_nonce: "7".into(),
                    permit2_deadline: case.wallet.now.load(Ordering::Relaxed) + 1800,
                    depositor: case.wallet.owner.to_string(),
                    coordinator: case.wallet.coordinator.to_string(),
                    lock_token: case.wallet.htlc.token.to_string(),
                    eip2612_permit: true,
                    tx_hash: None,
                    outcome: RelayFundingOutcome::Sending,
                }),
                evm_fund_started: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    case.mount_relay(0).await;
    let refused = case.fund(&options).await;
    assert!(reconcile_first(&refused), "{refused:?}");
    case.server.verify().await;
}

#[tokio::test]
async fn an_unsettled_relay_signature_blocks_other_funding_after_a_release() {
    use crate::storage::RelayFundingOutcome;
    fn relay_blocked<T: std::fmt::Debug>(result: &crate::Result<T>) -> bool {
        matches!(result, Err(Error::UnresolvedFunding(message)) if message.contains("reconcile_gasless_relay_funding"))
    }
    let case = RelayCase::new(|_| {}, |_| {}).await;
    for outcome in [
        RelayFundingOutcome::Sending,
        RelayFundingOutcome::Rejected,
        RelayFundingOutcome::Reverted,
        RelayFundingOutcome::Accepted,
        RelayFundingOutcome::Uncertain,
        RelayFundingOutcome::Expired,
    ] {
        // The flag was released by hand, as the generic unresolved-funding error suggests.
        case.client
            .update_swap_metadata(
                "test",
                SwapMetadata {
                    evm_fund_relay: Some(crate::storage::RelayFundingJournal {
                        permit2_nonce: "7".into(),
                        permit2_deadline: case.wallet.now.load(Ordering::Relaxed) + 1800,
                        depositor: case.wallet.owner.to_string(),
                        coordinator: case.wallet.coordinator.to_string(),
                        lock_token: case.wallet.htlc.token.to_string(),
                        eip2612_permit: true,
                        tx_hash: None,
                        outcome,
                    }),
                    evm_fund_started: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        let unsettled = outcome != RelayFundingOutcome::Expired;
        let gasless = case
            .client
            .fund_swap_gasless(
                "test",
                crate::aa::GaslessOpts::new(case.server.uri().parse().unwrap()),
            )
            .await;
        assert_eq!(
            relay_blocked(&gasless),
            unsettled,
            "{outcome:?}: {gasless:?}"
        );
        let wallet = case
            .client
            .fund_swap_with_wallet("test", &case.wallet, relay_pinned(100_000, vec![]))
            .await;
        assert_eq!(relay_blocked(&wallet), unsettled, "{outcome:?}: {wallet:?}");
    }
}

#[tokio::test]
async fn a_funding_hash_is_attached_only_once_no_relay_signature_can_fund() {
    use super::funding::UNRESOLVED_RELAY_FUNDING;
    use crate::storage::RelayFundingOutcome;
    let case = RelayCase::new(|_| {}, |_| {}).await;
    let stored = case.store.get_swap("test").unwrap().unwrap();
    let hash = B256::repeat_byte(9);
    for outcome in [
        RelayFundingOutcome::Sending,
        RelayFundingOutcome::Rejected,
        RelayFundingOutcome::Reverted,
        RelayFundingOutcome::Accepted,
        RelayFundingOutcome::Uncertain,
        RelayFundingOutcome::Expired,
    ] {
        // A fresh record each time, since metadata merges could not clear the hash.
        case.store.delete_swap("test").unwrap();
        case.store
            .store_swap(
                StoredSwap::new(
                    0,
                    stored.response.clone(),
                    SwapMetadata {
                        evm_fund_started: Some(true),
                        evm_fund_relay: Some(crate::storage::RelayFundingJournal {
                            permit2_nonce: "7".into(),
                            permit2_deadline: case.wallet.now.load(Ordering::Relaxed) + 1800,
                            depositor: case.wallet.owner.to_string(),
                            coordinator: case.wallet.coordinator.to_string(),
                            lock_token: case.wallet.htlc.token.to_string(),
                            eip2612_permit: true,
                            tx_hash: None,
                            outcome,
                        }),
                        ..Default::default()
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let result = case
            .client
            .attach_evm_funding_transaction("test", hash)
            .await;
        let attached = case
            .store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_fund_txid;
        if outcome == RelayFundingOutcome::Expired {
            result.unwrap();
            assert_eq!(attached, Some(hash.to_string()));
        } else {
            assert!(
                matches!(&result, Err(Error::UnresolvedFunding(message)) if message == UNRESOLVED_RELAY_FUNDING),
                "{outcome:?}: {result:?}"
            );
            assert_eq!(attached, None, "{outcome:?}");
        }
    }
}

#[tokio::test]
async fn only_a_funding_without_a_recorded_hash_is_unresolved() {
    use super::funding::UNRESOLVED_CCTP_FUNDING;
    use super::funding::UNRESOLVED_FUNDING;
    use crate::storage::BridgeOperation;
    let case = RelayCase::new(|_| {}, |_| {}).await;
    case.mount_relay(0).await;
    let stored = case.store.get_swap("test").unwrap().unwrap();
    let cctp_fund = |user_operation_hash: Option<B256>, complete: bool| {
        let operation = BridgeOperation {
            intent: json!({}),
            transaction_hash: None,
            user_operation_hash: user_operation_hash.map(|hash| hash.to_string()),
            complete,
        };
        Some(std::collections::BTreeMap::from([(
            "cctp_fund".to_string(),
            operation,
        )]))
    };
    let txid = Some(B256::repeat_byte(1).to_string());
    for (metadata, unresolved) in [
        (
            SwapMetadata {
                evm_fund_started: Some(true),
                ..Default::default()
            },
            Some(UNRESOLVED_FUNDING),
        ),
        (
            SwapMetadata {
                bridge_operations: cctp_fund(None, false),
                ..Default::default()
            },
            Some(UNRESOLVED_CCTP_FUNDING),
        ),
        (
            SwapMetadata {
                evm_fund_txid: txid.clone(),
                bridge_operations: cctp_fund(None, false),
                ..Default::default()
            },
            Some(UNRESOLVED_CCTP_FUNDING),
        ),
        (
            SwapMetadata {
                bridge_operations: cctp_fund(Some(B256::repeat_byte(2)), false),
                ..Default::default()
            },
            None,
        ),
        (
            SwapMetadata {
                bridge_operations: cctp_fund(None, true),
                ..Default::default()
            },
            None,
        ),
        (
            SwapMetadata {
                evm_fund_txid: txid.clone(),
                ..Default::default()
            },
            None,
        ),
    ] {
        // A fresh record each time, since metadata merges could not clear these again.
        case.store.delete_swap("test").unwrap();
        case.store
            .store_swap(
                StoredSwap::new(
                    0,
                    stored.response.clone(),
                    SwapMetadata {
                        evm_coordinator_address: stored.metadata.evm_coordinator_address.clone(),
                        ..metadata
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let relay = case.fund(&RelayFundingOptions::new(U256::from(300))).await;
        let wallet = case
            .client
            .fund_swap_with_wallet("test", &case.wallet, relay_pinned(100_000, vec![]))
            .await;
        let gasless = case
            .client
            .fund_swap_gasless(
                "test",
                crate::aa::GaslessOpts::new(case.server.uri().parse().unwrap()),
            )
            .await;
        for error in [
            relay.unwrap_err(),
            wallet.unwrap_err(),
            gasless.unwrap_err(),
        ] {
            let expected = match unresolved {
                Some(text) => {
                    matches!(&error, Error::UnresolvedFunding(message) if message == text)
                }
                None => {
                    matches!(&error, Error::InvalidSwap(message) if message.contains("inspect its receipt"))
                }
            };
            assert!(expected, "{error}");
        }
    }
    case.server.verify().await;
}

#[tokio::test]
async fn relay_revert_resigns_with_the_same_nonce() {
    use crate::storage::RelayFundingOutcome;
    let case = RelayCase::new(|_| {}, |_| {}).await;
    case.mount_relay(2).await;
    case.wallet
        .reverted_receipts
        .lock()
        .unwrap()
        .push(B256::repeat_byte(42));
    let options = RelayFundingOptions::new(U256::from(300));
    let reverted = case.fund(&options).await;
    assert!(
        matches!(&reverted, Err(Error::InvalidSwap(message)) if message.contains("reverted")),
        "{reverted:?}"
    );
    let metadata = case.metadata();
    assert_eq!(metadata.evm_fund_txid, None);
    assert_eq!(
        metadata.evm_fund_relay.unwrap().outcome,
        RelayFundingOutcome::Reverted
    );
    case.wallet.reverted_receipts.lock().unwrap().clear();
    case.fund(&options).await.unwrap();
    assert_eq!(case.body(0)["permit2_nonce"], case.body(1)["permit2_nonce"]);
    assert_eq!(case.journal().outcome, RelayFundingOutcome::Funded);
}

#[tokio::test]
async fn relay_timeout_says_whether_the_request_may_have_been_sent() {
    use crate::storage::RelayFundingOutcome;
    use wiremock::Mock;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    fn timed_out(result: &crate::Result<RelayFunding>, advice: &str) -> bool {
        matches!(result, Err(Error::Timeout(message)) if message.contains(advice))
    }
    let mut options = RelayFundingOptions::new(U256::from(300));
    options.timeout = Duration::from_secs(1);

    // Held before anything is persisted: the call can be repeated as it is.
    let case = RelayCase::new(|_| {}, |_| {}).await;
    case.mount_relay(1).await;
    Mock::given(method("GET"))
        .and(path("/swap/test/swap-and-lock-calldata-permit2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(&case.payload)
                .set_delay(Duration::from_secs(30)),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&case.server)
        .await;
    let result = case.fund_within(&options).await;
    assert!(timed_out(&result, "can be repeated"), "{result:?}");
    assert_eq!(case.metadata().evm_fund_relay, None);
    // No limit, as with `tokio::time::timeout`.
    options.timeout = Duration::MAX;
    case.fund_within(&options).await.unwrap();

    // Held waiting for the receipt of an accepted request.
    let case = RelayCase::new(|_| {}, |_| {}).await;
    case.wallet
        .hanging_receipts
        .lock()
        .unwrap()
        .push(B256::repeat_byte(42));
    case.mount_relay(1).await;
    options.timeout = Duration::from_secs(3);
    let result = case.fund_within(&options).await;
    assert!(timed_out(&result, "reconcile"), "{result:?}");
    assert_eq!(case.journal().outcome, RelayFundingOutcome::Accepted);
}

#[tokio::test]
async fn relay_kernel_delegate_uses_erc1271_envelope() {
    let kernel = crate::aa::kernel::KERNEL_DELEGATION_TARGET;
    let mut designator = vec![0xef, 0x01, 0x00];
    designator.extend_from_slice(kernel.as_slice());
    // (server hint, probed code, whether the envelope is expected)
    let hinted: fn(&mut Value) =
        |p| p["depositor_delegation"] = json!(crate::aa::kernel::KERNEL_DELEGATION_TARGET);
    let unhinted: fn(&mut Value) = |p| {
        p.as_object_mut().unwrap().remove("depositor_delegation");
    };
    for (edit, code, envelope) in [
        (hinted, None, true),
        (unhinted, Some(Bytes::from(designator.clone())), true),
        (unhinted, Some(Bytes::new()), false),
        (unhinted, None, false),
    ] {
        let case = RelayCase::new(|_| {}, edit).await;
        *case.wallet.code.lock().unwrap() = code;
        case.mount_relay(1).await;
        case.fund(&RelayFundingOptions::new(U256::from(300)))
            .await
            .unwrap();
        let body = case.body(0);
        let signature = signature_bytes(&body["permit2_signature"]);
        let permit = &body["eip2612_permit"];
        if !envelope {
            assert_eq!(signature.len(), 65);
            assert!(permit.get("v").is_some());
            continue;
        }
        let owner = case.wallet.owner;
        let wrapped = |digest| crate::aa::kernel::erc1271_wrapped_digest(digest, owner, 137);
        assert_eq!((signature.len(), signature[0]), (66, 0x00));
        assert_eq!(
            recovers(&signature[1..], wrapped(case.permit2_digest(&body))),
            owner
        );
        assert!(permit.get("v").is_none());
        let permit_signature = signature_bytes(&permit["signature"]);
        assert_eq!((permit_signature.len(), permit_signature[0]), (66, 0x00));
        assert_eq!(
            recovers(&permit_signature[1..], wrapped(case.permit_digest(&body))),
            owner
        );
    }
}

fn setup_direction(
    direction: &str,
    kind: EvmHtlcKind,
) -> (Client, Wallet, Arc<InMemorySwapStorage>) {
    setup_fixture(direction, direction.ends_with("_to_evm"), kind)
}
