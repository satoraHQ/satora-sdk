use super::funding::SwapCreated;
use super::funding::transaction;
use super::htlc::Coordinator;
use super::htlc::EvmHtlc;
use super::htlc::Htlc;
use super::htlc::SwapRedeemed;
use super::htlc::SwapRefunded;
use super::htlc::SwapRehashed;
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
    native_wei: AtomicU64,
    fee_wei: AtomicU64,
    /// Hashes the next broadcasts return, before falling back to one fixed hash.
    next_hashes: Mutex<Vec<B256>>,
    /// Coordinator `deposits` reads.
    deposit_reads: AtomicU64,
    /// Receipts served with these logs whatever was sent last.
    fixed_receipts: Mutex<std::collections::HashMap<B256, Vec<EvmLog>>>,
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
    fn sign_message<'a>(&'a self, message: &'a [u8]) -> WalletFuture<'a, Signature> {
        Box::pin(async move {
            self.signatures.fetch_add(1, Ordering::Relaxed);
            crate::aa::signing::sign_eip191_message(&self.secret, message)
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
                self.deposit_reads.fetch_add(1, Ordering::Relaxed);
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
            let mut next = self.next_hashes.lock().unwrap();
            Ok(if next.is_empty() {
                B256::repeat_byte(99)
            } else {
                next.remove(0)
            })
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
            if let Some(logs) = self.fixed_receipts.lock().unwrap().get(&hash) {
                return Ok(EvmReceipt {
                    transaction_hash: hash,
                    success: true,
                    block_number: 10,
                    logs: logs.clone(),
                });
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
                // The settled lock is the one the last sent call names: after a
                // rehash the refund targets the moved terms, not the funding's.
                let last = self.sent.lock().unwrap().last().cloned();
                let terms = |hash: B256, timelock: U256| EvmHtlc {
                    hash_lock: hash,
                    timelock: timelock.to::<u64>(),
                    ..h.clone()
                };
                let rehash = last.as_ref().and_then(|tx| {
                    Htlc::rehashCall::abi_decode(&tx.data)
                        .ok()
                        .map(|c| (c.preimageHash, c.timelock, c.newPreimageHash, c.newTimelock))
                        .or_else(|| {
                            super::refund::NativeHtlc::rehashCall::abi_decode(&tx.data)
                                .ok()
                                .map(|c| {
                                    (c.preimageHash, c.timelock, c.newPreimageHash, c.newTimelock)
                                })
                        })
                });
                if let Some((hash, timelock, new_hash, new_timelock)) = rehash {
                    event_log(
                        SwapRehashed {
                            preimageHash: hash,
                            key: terms(hash, timelock).key(),
                            newKey: terms(new_hash, new_timelock).key(),
                            newPreimageHash: new_hash,
                        },
                        h.address,
                    )
                } else {
                    let refunded = last
                        .as_ref()
                        .and_then(|tx| {
                            Htlc::refundCall::abi_decode(&tx.data)
                                .ok()
                                .map(|c| (c.preimageHash, c.timelock))
                                .or_else(|| {
                                    super::refund::NativeHtlc::refundCall::abi_decode(&tx.data)
                                        .ok()
                                        .map(|c| (c.preimageHash, c.timelock))
                                })
                                .or_else(|| {
                                    Htlc::refundBySigCall::abi_decode(&tx.data)
                                        .ok()
                                        .map(|c| (c.preimageHash, c.timelock))
                                })
                        })
                        .map(|(hash, timelock)| terms(hash, timelock))
                        .unwrap_or_else(|| h.clone());
                    event_log(
                        SwapRefunded {
                            preimageHash: refunded.hash_lock,
                            key: refunded.key(),
                        },
                        h.address,
                    )
                }
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
    fn native_balance(&self) -> WalletFuture<'_, U256> {
        Box::pin(async { Ok(U256::from(self.native_wei.load(Ordering::Relaxed))) })
    }
    fn transaction_fee<'a>(&'a self, _: &'a EvmTransaction) -> WalletFuture<'a, U256> {
        Box::pin(async { Ok(U256::from(self.fee_wei.load(Ordering::Relaxed))) })
    }
}

/// A real LND hold invoice paying the hash of the preimage at key index 0.
fn invoice_paying_key_index_0() -> String {
    fixture_invoice("lnd_hold_index_0").0
}

/// A fixture invoice and the payment hash it commits to.
fn fixture_invoice(name: &str) -> (String, B256) {
    let vectors: Value =
        serde_json::from_str(include_str!("../../../fixtures/lightning-invoices.json")).unwrap();
    let invoice = &vectors["invoices"][name];
    (
        invoice["bolt11"].as_str().unwrap().to_owned(),
        invoice["payment_hash"].as_str().unwrap().parse().unwrap(),
    )
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
        native_wei: AtomicU64::new(1_000_000),
        fee_wei: AtomicU64::new(21_000),
        next_hashes: Mutex::new(vec![]),
        deposit_reads: AtomicU64::new(0),
        fixed_receipts: Mutex::default(),
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

fn sent_transfer(wallet: &Wallet, index: usize) -> (Address, Address, U256) {
    let sent = wallet.sent.lock().unwrap()[index].clone();
    let transfer = super::recovery::RecoveryToken::transferCall::abi_decode(&sent.data).unwrap();
    (sent.to, transfer.to, transfer.amount)
}

#[tokio::test]
async fn a_sweep_the_depositor_cannot_pay_for_reserves_nothing() {
    let (client, wallet, store) = setup(false);
    let destination = Address::repeat_byte(80);
    for native in [0, 20_999] {
        wallet.native_wei.store(native, Ordering::Relaxed);
        let result = client
            .sweep_with_wallet("test", &wallet, None, destination)
            .await;
        assert!(
            matches!(&result, Err(Error::InsufficientGas(_))),
            "{result:?}"
        );
    }
    let metadata = store.get_swap("test").unwrap().unwrap().metadata;
    assert_eq!(metadata.evm_recovery_started, None);
    assert_eq!(metadata.evm_recovery_intent, None);
    assert!(wallet.sent.lock().unwrap().is_empty());
    wallet.native_wei.store(21_000, Ordering::Relaxed);
    client
        .sweep_with_wallet("test", &wallet, None, destination)
        .await
        .unwrap();
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_sweep_moves_the_whole_balance_and_a_repeat_returns_its_receipt() {
    let (client, wallet, _) = setup(false);
    let destination = Address::repeat_byte(80);
    let sweep = || client.sweep_with_wallet("test", &wallet, None, destination);
    wallet.token_balance.store(12_345, Ordering::Relaxed);
    let first = sweep().await.unwrap();
    // `None` sweeps the swap's source token.
    assert_eq!(
        sent_transfer(&wallet, 0),
        (wallet.htlc.token, destination, U256::from(12_345))
    );
    // The transfer moved the balance, so a repeat only verifies it.
    wallet.token_balance.store(0, Ordering::Relaxed);
    assert_eq!(
        sweep().await.unwrap().transaction_hash,
        first.transaction_hash
    );
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
    // A later deposit is swept too, even one of the same amount.
    wallet.token_balance.store(12_345, Ordering::Relaxed);
    sweep().await.unwrap();
    assert_eq!(wallet.sent.lock().unwrap().len(), 2);
    let other = Address::repeat_byte(81);
    wallet.token_balance.store(0, Ordering::Relaxed);
    let empty = client.sweep_with_wallet("test", &wallet, None, other).await;
    assert!(
        matches!(&empty, Err(Error::InvalidSwap(message)) if message.contains("holds none")),
        "{empty:?}"
    );
}

#[tokio::test]
async fn a_sweep_whose_receipt_was_lost_verifies_the_reserved_amount() {
    let (client, wallet, store) = setup(false);
    let destination = Address::repeat_byte(80);
    let sweep = || client.sweep_with_wallet("test", &wallet, None, destination);
    wallet.token_balance.store(500, Ordering::Relaxed);
    let sent = B256::repeat_byte(99);
    wallet.missing_receipts.lock().unwrap().push(sent);
    assert!(matches!(sweep().await, Err(Error::Timeout(_))));
    let metadata = store.get_swap("test").unwrap().unwrap().metadata;
    assert_eq!(metadata.evm_recovery_txid, Some(sent.to_string()));
    // The transfer mined meanwhile, so only the reservation knows its amount.
    wallet.missing_receipts.lock().unwrap().clear();
    wallet.token_balance.store(0, Ordering::Relaxed);
    assert_eq!(sweep().await.unwrap().transaction_hash, sent);
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
    let other = client
        .sweep_with_wallet("test", &wallet, None, Address::repeat_byte(81))
        .await;
    assert!(
        matches!(&other, Err(Error::InvalidSwap(message)) if message.contains("holds none")),
        "{other:?}"
    );
}

#[tokio::test]
async fn a_sweep_reserved_without_a_hash_is_sent_again() {
    let (client, wallet, store) = setup(false);
    let (token, destination) = (wallet.htlc.token, Address::repeat_byte(80));
    let sweep = |destination| client.sweep_with_wallet("test", &wallet, None, destination);
    wallet.token_balance.store(500, Ordering::Relaxed);
    wallet.fail_send.store(true, Ordering::Relaxed);
    assert!(matches!(sweep(destination).await, Err(Error::Transport(_))));
    wallet.fail_send.store(false, Ordering::Relaxed);
    let elsewhere = sweep(Address::repeat_byte(81)).await;
    assert!(
        matches!(&elsewhere, Err(Error::InvalidSwap(message)) if message.contains(&format!("sweep {token} to {destination} first"))),
        "{elsewhere:?}"
    );
    let explicit = client
        .recover_evm_deposit_with_wallet(
            "test",
            &wallet,
            DepositRecovery {
                token,
                destination,
                amount: U256::from(500),
                timeout: Duration::from_secs(5),
            },
        )
        .await;
    assert!(
        matches!(&explicit, Err(Error::InvalidSwap(message)) if message.contains("unresolved")),
        "{explicit:?}"
    );
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
    let receipt = sweep(destination).await.unwrap();
    assert_eq!(
        sent_transfer(&wallet, 1),
        (token, destination, U256::from(500))
    );
    let metadata = store.get_swap("test").unwrap().unwrap().metadata;
    assert_eq!(
        metadata.evm_recovery_settled,
        Some(receipt.transaction_hash.to_string())
    );
    assert_eq!(metadata.evm_recovery_started, Some(false));
}

#[tokio::test]
async fn a_sweep_reserved_without_a_hash_is_released_once_the_balance_is_gone() {
    let (client, wallet, store) = setup(false);
    let (token, destination) = (wallet.htlc.token, Address::repeat_byte(80));
    let sweep = |destination| client.sweep_with_wallet("test", &wallet, None, destination);
    wallet.token_balance.store(500, Ordering::Relaxed);
    wallet.fail_send.store(true, Ordering::Relaxed);
    assert!(matches!(sweep(destination).await, Err(Error::Transport(_))));
    // The lost transfer mined.
    wallet.fail_send.store(false, Ordering::Relaxed);
    wallet.token_balance.store(0, Ordering::Relaxed);
    let gone = sweep(destination).await;
    assert!(
        matches!(&gone, Err(Error::InvalidSwap(message)) if message.contains("holds none")),
        "{gone:?}"
    );
    let metadata = store.get_swap("test").unwrap().unwrap().metadata;
    assert_eq!(metadata.evm_recovery_started, Some(false));
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
    let other = Address::repeat_byte(81);
    wallet.token_balance.store(300, Ordering::Relaxed);
    sweep(other).await.unwrap();
    assert_eq!(sent_transfer(&wallet, 1), (token, other, U256::from(300)));
}

#[tokio::test]
async fn a_reverted_sweep_is_released_and_sent_again() {
    let (client, wallet, store) = setup(false);
    let destination = Address::repeat_byte(80);
    let reverted = B256::repeat_byte(55);
    wallet.next_hashes.lock().unwrap().push(reverted);
    wallet.reverted_receipts.lock().unwrap().push(reverted);
    let result = client
        .sweep_with_wallet("test", &wallet, None, destination)
        .await;
    assert!(
        matches!(&result, Err(Error::InvalidSwap(message)) if message.contains("reverted")),
        "{result:?}"
    );
    let metadata = store.get_swap("test").unwrap().unwrap().metadata;
    assert_eq!(metadata.evm_recovery_started, Some(false));
    assert_eq!(metadata.evm_recovery_reverted, Some(reverted.to_string()));
    let receipt = client
        .sweep_with_wallet("test", &wallet, None, destination)
        .await
        .unwrap();
    assert_eq!(receipt.transaction_hash, B256::repeat_byte(99));
    assert_eq!(wallet.sent.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn deposit_spends_wait_until_no_relay_signature_can_pull_the_deposit() {
    use super::funding::RELAY_SIGNATURE_SPENDABLE;
    use crate::storage::RelayFundingOutcome;
    let destination = Address::repeat_byte(80);
    for outcome in [
        RelayFundingOutcome::Sending,
        RelayFundingOutcome::Rejected,
        RelayFundingOutcome::Reverted,
        RelayFundingOutcome::Accepted,
        RelayFundingOutcome::Uncertain,
        RelayFundingOutcome::Funded,
        RelayFundingOutcome::Expired,
    ] {
        let (client, wallet, _) = setup(false);
        let deadline = wallet.now.load(Ordering::Relaxed) + 1800;
        client
            .update_swap_metadata(
                "test",
                SwapMetadata {
                    evm_fund_relay: Some(crate::storage::RelayFundingJournal {
                        permit2_nonce: "7".into(),
                        permit2_deadline: deadline,
                        depositor: wallet.owner.to_string(),
                        coordinator: wallet.coordinator.to_string(),
                        lock_token: wallet.htlc.token.to_string(),
                        eip2612_permit: true,
                        tx_hash: None,
                        outcome,
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        wallet.now.store(deadline + 120, Ordering::Relaxed);
        let recovered = client
            .recover_evm_deposit_with_wallet(
                "test",
                &wallet,
                DepositRecovery {
                    token: wallet.htlc.token,
                    destination,
                    amount: U256::from(100),
                    timeout: Duration::from_secs(5),
                },
            )
            .await
            .map(|_| ());
        let swept = client
            .sweep_with_wallet("test", &wallet, None, destination)
            .await
            .map(|_| ());
        if matches!(
            outcome,
            RelayFundingOutcome::Funded | RelayFundingOutcome::Expired
        ) {
            recovered.unwrap();
            swept.unwrap();
            continue;
        }
        for result in [recovered, swept] {
            assert!(
                matches!(&result, Err(Error::UnresolvedFunding(message)) if message == RELAY_SIGNATURE_SPENDABLE),
                "{outcome:?}: {result:?}"
            );
        }
        assert!(wallet.sent.lock().unwrap().is_empty(), "{outcome:?}");
        wallet.now.store(deadline + 121, Ordering::Relaxed);
        client
            .sweep_with_wallet("test", &wallet, None, destination)
            .await
            .unwrap();
    }
}

/// Records the swap as funded through the relay, whose journal names its lock token.
fn record_relay_funding(client: &Client, wallet: &Wallet) {
    client
        .update_swap_metadata(
            "test",
            SwapMetadata {
                evm_fund_relay: Some(crate::storage::RelayFundingJournal {
                    permit2_nonce: "7".into(),
                    permit2_deadline: 1,
                    depositor: wallet.owner.to_string(),
                    coordinator: wallet.coordinator.to_string(),
                    lock_token: wallet.htlc.token.to_string(),
                    eip2612_permit: true,
                    tx_hash: Some(wallet.funding_hash.to_string()),
                    outcome: crate::storage::RelayFundingOutcome::Funded,
                }),
                ..Default::default()
            },
        )
        .unwrap();
}

#[tokio::test]
async fn an_htlc_refund_waits_for_the_timelock_and_gas_and_verifies_on_retry() {
    let (client, wallet, store) = setup(false);
    let refund = || client.refund_htlc_with_wallet("test", &wallet, Duration::from_secs(5));
    // An EVM to Arkade response names no lock token; the relay journal does.
    let unknown = refund().await;
    assert!(
        matches!(&unknown, Err(Error::InvalidSwap(message)) if message.contains("lock token")),
        "{unknown:?}"
    );
    record_relay_funding(&client, &wallet);
    wallet
        .now
        .store(wallet.htlc.timelock - 1, Ordering::Relaxed);
    let early = refund().await;
    assert!(
        matches!(&early, Err(Error::InvalidSwap(message)) if message.contains("timelock")),
        "{early:?}"
    );
    wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
    wallet.native_wei.store(0, Ordering::Relaxed);
    let poor = refund().await;
    assert!(matches!(&poor, Err(Error::InsufficientGas(_))), "{poor:?}");
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_refund_started,
        None
    );
    assert!(wallet.sent.lock().unwrap().is_empty());
    wallet.native_wei.store(1_000_000, Ordering::Relaxed);
    // The lock token, not the source token, is what the refund pays back.
    let source = Address::repeat_byte(9);
    rewrite_response(&store, |r| r["source_token"]["token_id"] = json!(source));
    assert_eq!(refund().await.unwrap().lock_token, wallet.htlc.token);
    {
        let sent = wallet.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to, wallet.coordinator);
        assert!(
            sent[0]
                .data
                .starts_with(&Coordinator::refundToCall::SELECTOR)
        );
    }
    refund().await.unwrap();
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn an_htlc_refund_reserved_without_a_hash_is_sent_again_while_the_htlc_is_active() {
    let (client, wallet, store) = setup(false);
    let refund = || client.refund_htlc_with_wallet("test", &wallet, Duration::from_secs(5));
    record_relay_funding(&client, &wallet);
    wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
    wallet.fail_send.store(true, Ordering::Relaxed);
    assert!(matches!(refund().await, Err(Error::Transport(_))));
    wallet.fail_send.store(false, Ordering::Relaxed);
    let receipt = refund().await.unwrap().receipt;
    {
        let sent = wallet.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].to, wallet.coordinator);
        assert_eq!(sent[1].data, sent[0].data);
    }
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
async fn an_htlc_refund_reserved_without_a_hash_is_not_sent_again_once_it_settled() {
    let (client, wallet, _) = setup(false);
    let refund = || client.refund_htlc_with_wallet("test", &wallet, Duration::from_secs(5));
    record_relay_funding(&client, &wallet);
    wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
    wallet.fail_send.store(true, Ordering::Relaxed);
    assert!(matches!(refund().await, Err(Error::Transport(_))));
    // The lost refund mined: the HTLC is settled and the coordinator dropped its deposit.
    wallet.fail_send.store(false, Ordering::Relaxed);
    wallet.settled.store(true, Ordering::Relaxed);
    let settled = refund().await;
    assert!(
        matches!(&settled, Err(Error::InvalidSwap(message)) if message.contains("refunded or claimed")),
        "{settled:?}"
    );
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_reverted_htlc_refund_is_released_and_sent_again() {
    let (client, wallet, store) = setup(false);
    let refund = || client.refund_htlc_with_wallet("test", &wallet, Duration::from_secs(5));
    record_relay_funding(&client, &wallet);
    wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
    let reverted = B256::repeat_byte(55);
    wallet.next_hashes.lock().unwrap().push(reverted);
    wallet.reverted_receipts.lock().unwrap().push(reverted);
    let result = refund().await;
    assert!(
        matches!(&result, Err(Error::InvalidSwap(message)) if message.contains("reverted")),
        "{result:?}"
    );
    let metadata = store.get_swap("test").unwrap().unwrap().metadata;
    assert_eq!(metadata.evm_refund_started, Some(false));
    assert_eq!(metadata.evm_refund_reverted, Some(reverted.to_string()));
    let receipt = refund().await.unwrap().receipt;
    assert_eq!(receipt.transaction_hash, B256::repeat_byte(99));
    assert_eq!(wallet.sent.lock().unwrap().len(), 2);
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
async fn a_lost_refund_after_a_reverted_one_can_be_attached() {
    let (client, wallet, _) = setup(false);
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(5),
    };
    let refund = || client.refund_evm_with_wallet("test", &wallet, options.clone());
    wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
    let reverted = B256::repeat_byte(55);
    wallet.next_hashes.lock().unwrap().push(reverted);
    wallet.reverted_receipts.lock().unwrap().push(reverted);
    assert!(refund().await.is_err());
    wallet.fail_send.store(true, Ordering::Relaxed);
    assert!(matches!(refund().await, Err(Error::Transport(_))));
    let unresolved = refund().await;
    assert!(
        matches!(&unresolved, Err(Error::InvalidSwap(message)) if message.contains("unresolved")),
        "{unresolved:?}"
    );
    let lost = B256::repeat_byte(77);
    client
        .attach_evm_refund_transaction("test", lost)
        .await
        .unwrap();
    assert_eq!(refund().await.unwrap().transaction_hash, lost);
    assert_eq!(wallet.sent.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn depositor_balance_reads_the_derived_depositor_and_the_source_token() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::Request;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_partial_json;
    let (client, wallet, _) = setup(false);
    let node = MockServer::start().await;
    for (method, result) in [
        ("eth_chainId", json!("0x89")),
        ("eth_getBalance", json!("0x2a")),
        ("eth_call", json!(format!("0x{:064x}", 12_345))),
    ] {
        Mock::given(body_partial_json(json!({ "method": method })))
            .respond_with(move |request: &Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc": "2.0", "id": body["id"], "result": result}))
            })
            .mount(&node)
            .await;
    }
    let url: url::Url = node.uri().parse().unwrap();
    assert_eq!(
        client
            .depositor_balance("test", None, Some(&url))
            .await
            .unwrap(),
        DepositorBalance {
            token_units: U256::from(12_345),
            native_wei: U256::from(42),
        }
    );
    let bodies: Vec<Value> = node
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect();
    let param = |method: &str| {
        bodies.iter().find(|body| body["method"] == method).unwrap()["params"][0].clone()
    };
    let address = |value: &Value| value.as_str().unwrap().parse::<Address>().unwrap();
    assert_eq!(address(&param("eth_getBalance")), wallet.owner);
    assert_eq!(address(&param("eth_call")["to"]), wallet.htlc.token);
    let (claimer, _, _) = setup(true);
    let refused = claimer.depositor_balance("test", None, Some(&url)).await;
    assert!(
        matches!(&refused, Err(Error::InvalidSwap(message)) if message.contains("EVM-sourced")),
        "{refused:?}"
    );
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

/// A malicious server names the Permit2 spender, the lock token and the calls the
/// coordinator runs. Each must be bound to the stored swap before anything is signed.
#[tokio::test]
async fn gasless_funding_refuses_a_payload_not_bound_to_the_stored_swap() {
    use crate::aa::abi::Call;
    use alloy::primitives::keccak256;
    use alloy::sol_types::SolValue;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let (_, wallet, fixture) = setup_fixture("evm_to_bitcoin", false, EvmHtlcKind::Erc20);
    let response = fixture.get_swap("test").unwrap().unwrap().response;
    let attacker = Address::repeat_byte(0x66);
    let dex = Address::repeat_byte(0x99);
    let token = wallet.htlc.token;
    let pinned = FundingOptions {
        coordinator: wallet.coordinator,
        htlc: wallet.htlc.address,
        lock_token: token,
        max_source_amount: U256::from(100_000),
        allowed_call_targets: vec![],
        timeout: Duration::from_secs(10),
    };
    let transfer = |to: Address, amount: u64| Call {
        target: token,
        value: U256::ZERO,
        callData: super::recovery::RecoveryToken::transferCall {
            to,
            amount: U256::from(amount),
        }
        .abi_encode()
        .into(),
    };
    let dex_call = Call {
        target: dex,
        value: U256::ZERO,
        callData: vec![0x12, 0x34, 0x56, 0x78].into(),
    };
    let payload = |calls: Vec<Call>| {
        json!({
            "coordinator_address": wallet.coordinator,
            "htlc_kind": "erc20",
            "htlc_version": 6,
            "permit2_address": crate::aa::permit2::PERMIT2_ADDRESS,
            "source_token_address": token,
            "source_amount": "100000",
            "lock_token_address": token,
            "preimage_hash": wallet.htlc.hash_lock,
            "claim_address": wallet.htlc.claimer,
            "timelock": wallet.htlc.timelock,
            "calls": calls.iter().map(|call| json!({
                "target": call.target,
                "value": call.value.to_string(),
                "call_data": call.callData,
            })).collect::<Vec<_>>(),
            "calls_hash": keccak256(calls.abi_encode()),
            "aa": {
                "entry_point": crate::aa::userop::ENTRY_POINT_V07,
                "delegation_target": crate::aa::kernel::KERNEL_DELEGATION_TARGET,
            },
        })
    };
    let mut served_attacker_coordinator = serde_json::to_value(&response).unwrap();
    served_attacker_coordinator["evm_coordinator_address"] = json!(attacker);
    let mut attacker_spender = payload(vec![]);
    attacker_spender["coordinator_address"] = json!(attacker);
    let mut unknown_lock_token = payload(vec![]);
    unknown_lock_token["lock_token_address"] = json!(attacker);
    let mut fee = payload(vec![transfer(wallet.htlc.claimer, 500)]);
    fee["relay_fee"] = json!("500");
    let mut over_input = payload(vec![]);
    over_input["source_amount"] = json!("100001");
    let refused = |message: &'static str| Some(message);
    let cases: Vec<(&str, Value, Value, crate::aa::GaslessOpts, Option<&str>)> = vec![
        (
            "honest",
            serde_json::to_value(&response).unwrap(),
            payload(vec![]),
            Default::default(),
            None,
        ),
        (
            "the server's swap names another coordinator",
            served_attacker_coordinator.clone(),
            attacker_spender.clone(),
            Default::default(),
            refused("server swap details differ"),
        ),
        (
            "the spender is not the swap's coordinator",
            serde_json::to_value(&response).unwrap(),
            attacker_spender,
            Default::default(),
            refused("does not match the swap"),
        ),
        (
            "unknown lock token",
            serde_json::to_value(&response).unwrap(),
            unknown_lock_token,
            Default::default(),
            refused("does not match the swap"),
        ),
        (
            "more than the stored input",
            serde_json::to_value(&response).unwrap(),
            over_input,
            Default::default(),
            refused("exceeds the approved limit"),
        ),
        (
            "the input sent to the attacker",
            serde_json::to_value(&response).unwrap(),
            payload(vec![dex_call.clone(), transfer(attacker, 100_000)]),
            Default::default(),
            refused("more than the fee and an approval"),
        ),
        (
            "the input approved to the attacker under a pin",
            serde_json::to_value(&response).unwrap(),
            payload(vec![Call {
                target: token,
                value: U256::ZERO,
                callData: crate::aa::abi::IERC20::approveCall {
                    spender: attacker,
                    amount: U256::from(100_000),
                }
                .abi_encode()
                .into(),
            }]),
            crate::aa::GaslessOpts::default().with_funding(pinned.clone()),
            refused("unapproved spender"),
        ),
        (
            "a call target outside the pinned list",
            serde_json::to_value(&response).unwrap(),
            payload(vec![dex_call.clone()]),
            crate::aa::GaslessOpts::default().with_funding(pinned.clone()),
            refused("unapproved call target"),
        ),
        (
            "a listed call target",
            serde_json::to_value(&response).unwrap(),
            payload(vec![dex_call.clone()]),
            crate::aa::GaslessOpts::default().with_funding(FundingOptions {
                allowed_call_targets: vec![dex],
                ..pinned.clone()
            }),
            None,
        ),
        (
            "a pinned coordinator other than the swap's",
            serde_json::to_value(&response).unwrap(),
            payload(vec![]),
            crate::aa::GaslessOpts::default().with_funding(FundingOptions {
                coordinator: attacker,
                ..pinned.clone()
            }),
            refused("does not match the swap"),
        ),
        (
            "a relay fee over the cap",
            serde_json::to_value(&response).unwrap(),
            fee.clone(),
            Default::default(),
            refused("relay fee exceeds the limit"),
        ),
        (
            "a relay fee within the cap",
            serde_json::to_value(&response).unwrap(),
            fee,
            crate::aa::GaslessOpts::default().with_max_relay_fee(U256::from(500)),
            None,
        ),
    ];
    for (name, served, funding, opts, expected) in cases {
        let store = Arc::new(InMemorySwapStorage::new());
        store
            .store_swap(StoredSwap::new(0, response.clone(), SwapMetadata::default()).unwrap())
            .unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/swap/test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&served))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/aa/config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "entry_point": crate::aa::userop::ENTRY_POINT_V07,
                "delegation_target": crate::aa::kernel::KERNEL_DELEGATION_TARGET,
                "permit2": crate::aa::permit2::PERMIT2_ADDRESS,
                "bundler": {"url": server.uri(), "casing": "camel"},
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/swap/test/swap-and-lock-calldata-userop"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&funding))
            .mount(&server)
            .await;
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .build()
            .unwrap();
        let opts = crate::aa::GaslessOpts {
            node_rpc_url: Some(server.uri().parse().unwrap()),
            ..opts
        };
        let result = client.fund_swap_gasless("test", opts).await;
        // The node and bundler share the mock's root: any JSON-RPC request there means
        // the payload was accepted and the flow moved on towards signing.
        let rpc = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|request| request.method.as_str() == "POST");
        match expected {
            None => assert!(rpc, "{name}: {result:?}"),
            Some(message) => {
                assert!(
                    matches!(&result, Err(Error::InvalidSwap(m)) if m.contains(message)),
                    "{name}: {result:?}"
                );
                assert!(!rpc, "{name}: reached the node or bundler");
            }
        }
        assert_eq!(
            store
                .get_swap("test")
                .unwrap()
                .unwrap()
                .metadata
                .evm_fund_started,
            None,
            "{name}"
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
    // A CCTP payout the SDK could bridge itself is still refused off Arbitrum,
    // and for a token other than the destination's native USDC.
    for (chain, token) in [
        ("Polygon", "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359"),
        ("Ethereum", "0x0000000000000000000000000000000000000001"),
        ("Tron", "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t"),
    ] {
        rewrite_response(&store, |r| {
            r["bridge_target_chain"] = json!(chain);
            r["bridge_target_token_address"] = json!(token);
        });
        let NotSent(error) = auto_claim(&claimer).await else {
            panic!("a {chain} payout the SDK cannot verify is refused");
        };
        assert!(error.to_string().contains("bridge"), "{chain}: {error}");
    }
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
                self.wallet.owner,
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
async fn relay_funding_is_proven_under_the_lock_sender_of_its_generation() {
    use crate::storage::RelayFundingOutcome;
    use wiremock::Mock;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    // v6: the coordinator is the sender and must hold the owner's deposit.
    // v7: the depositor is the sender and the coordinator keeps no deposit.
    for version in [6, 7] {
        let edit_response = |r: &mut Value| r["evm_htlc_version"] = json!(version);
        let edit_payload = |p: &mut Value| {
            p["htlc_kind"] = json!("erc20");
            p["htlc_version"] = json!(version);
        };
        let new_case = || async {
            let mut case = RelayCase::new(edit_response, edit_payload).await;
            if version == 7 {
                case.wallet.htlc.sender = case.wallet.owner;
                case.wallet.version.store(7, Ordering::Relaxed);
                // The HTLC's pull of the lock from the coordinator, after SwapCreated.
                let h = case.wallet.htlc.clone();
                case.wallet.extra_logs.push(event_log(
                    super::recovery::Transfer {
                        from: case.wallet.coordinator,
                        to: h.address,
                        value: h.amount,
                    },
                    h.token,
                ));
            }
            case
        };
        let sender = |case: &RelayCase| {
            if version == 7 {
                case.wallet.owner
            } else {
                case.wallet.coordinator
            }
        };

        // Funding answered by the relay.
        let case = new_case().await;
        case.mount_relay(1).await;
        let funded = case
            .fund(&RelayFundingOptions::new(U256::from(300)))
            .await
            .unwrap();
        assert_eq!(funded.htlc.sender, sender(&case), "v{version}");
        assert_eq!(
            case.wallet.deposit_reads.load(Ordering::Relaxed) > 0,
            version == 6,
            "v{version}"
        );

        // A lost answer, reconciled from the server's funding hash.
        let case = new_case().await;
        case.relay.statuses.lock().unwrap().push(502);
        case.mount_relay(1).await;
        assert!(
            case.fund(&RelayFundingOptions::new(U256::from(300)))
                .await
                .is_err()
        );
        let journal = case.journal();
        assert_eq!(journal.outcome, RelayFundingOutcome::Uncertain);
        case.wallet
            .spent_nonces
            .lock()
            .unwrap()
            .push(U256::from_str_radix(&journal.permit2_nonce, 10).unwrap());
        let mut response =
            serde_json::to_value(case.store.get_swap("test").unwrap().unwrap().response).unwrap();
        response["evm_fund_txid"] = json!(B256::repeat_byte(42));
        Mock::given(method("GET"))
            .and(path("/swap/test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .with_priority(1)
            .mount(&case.server)
            .await;
        let RelayFundingState::Funded(funded) = case.reconcile().await.unwrap() else {
            panic!("v{version}: expected the server's hash to prove the funding")
        };
        assert_eq!(funded.htlc.sender, sender(&case), "v{version}");
        assert_eq!(
            case.wallet.deposit_reads.load(Ordering::Relaxed) > 0,
            version == 6,
            "v{version}"
        );
        assert_eq!(case.journal().outcome, RelayFundingOutcome::Funded);
    }
}

/// On v7 anyone can create a lock naming the depositor, so a receipt whose matching
/// `SwapCreated` was not paid for by the coordinator must not be accepted as the
/// relay's funding, even when the coordinator funded another lock in the same tx.
#[tokio::test]
async fn v7_relay_funding_rejects_a_decoy_lock_the_coordinator_did_not_pay_for() {
    use crate::storage::RelayFundingOutcome;
    use wiremock::Mock;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let edit_response = |r: &mut Value| r["evm_htlc_version"] = json!(7);
    let edit_payload = |p: &mut Value| {
        p["htlc_kind"] = json!("erc20");
        p["htlc_version"] = json!(7);
    };
    // Lock-token transfers logged after the matching SwapCreated, in order.
    // `None` as `from` stands for the coordinator.
    type Pulls = Vec<(Option<Address>, Option<Address>, Option<U256>)>;
    let new_case = |pulls: Pulls| async move {
        let mut case = RelayCase::new(edit_response, edit_payload).await;
        case.wallet.htlc.sender = case.wallet.owner;
        case.wallet.version.store(7, Ordering::Relaxed);
        let h = case.wallet.htlc.clone();
        for (from, token, value) in pulls {
            case.wallet.extra_logs.push(event_log(
                super::recovery::Transfer {
                    from: from.unwrap_or(case.wallet.coordinator),
                    to: h.address,
                    value: value.unwrap_or(h.amount),
                },
                token.unwrap_or(h.token),
            ));
        }
        case
    };
    let relay = Some(Address::repeat_byte(0x77));
    let decoys: Vec<Pulls> = vec![
        // No pull at all.
        vec![],
        // Paid by the relay that created it.
        vec![(relay, None, None)],
        // The decoy's own pull, then the coordinator funding another swap.
        vec![(relay, None, None), (None, None, Some(U256::from(1_000)))],
        // A coordinator transfer of another token.
        vec![(None, Some(Address::repeat_byte(0x78)), None)],
        // A coordinator transfer of another amount.
        vec![(None, None, Some(U256::from(1)))],
    ];

    // Funding answered by the relay.
    for pulls in decoys.clone() {
        let case = new_case(pulls.clone()).await;
        case.mount_relay(1).await;
        let error = case
            .fund(&RelayFundingOptions::new(U256::from(300)))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("coordinator paying"),
            "{pulls:?}: {error}"
        );
        assert_ne!(case.journal().outcome, RelayFundingOutcome::Funded);
    }

    // A lost answer, reconciled from a decoy hash the server reports.
    for pulls in decoys {
        let case = new_case(pulls.clone()).await;
        case.relay.statuses.lock().unwrap().push(502);
        case.mount_relay(1).await;
        assert!(
            case.fund(&RelayFundingOptions::new(U256::from(300)))
                .await
                .is_err()
        );
        let journal = case.journal();
        case.wallet
            .spent_nonces
            .lock()
            .unwrap()
            .push(U256::from_str_radix(&journal.permit2_nonce, 10).unwrap());
        let mut response =
            serde_json::to_value(case.store.get_swap("test").unwrap().unwrap().response).unwrap();
        response["evm_fund_txid"] = json!(B256::repeat_byte(42));
        Mock::given(method("GET"))
            .and(path("/swap/test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .with_priority(1)
            .mount(&case.server)
            .await;
        assert!(
            !matches!(case.reconcile().await, Ok(RelayFundingState::Funded(_))),
            "{pulls:?}: a decoy receipt reconciled as the funding"
        );
        assert_ne!(case.journal().outcome, RelayFundingOutcome::Funded);
    }
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

/// The stored swap as a HTLCErc20 v7 / HTLCNative v2 lock: the owner is the
/// lock's sender, the coordinator only the contract the funding went through.
fn make_depositor_sender(wallet: &mut Wallet, store: &InMemorySwapStorage) {
    let mut stored = store.get_swap("test").unwrap().unwrap();
    let mut value = serde_json::to_value(&stored.response).unwrap();
    value["evm_htlc_version"] = json!(if wallet.htlc.token.is_zero() { 2 } else { 7 });
    stored.response = serde_json::from_value(value).unwrap();
    store.store_swap(stored).unwrap();
    wallet.htlc.sender = wallet.owner;
    wallet.version.store(7, Ordering::Relaxed);
}

#[tokio::test]
async fn depositor_sender_locks_are_refunded_on_the_htlc_despite_the_coordinator() {
    for kind in [EvmHtlcKind::Erc20, EvmHtlcKind::Native] {
        let (client, mut wallet, store) = setup_kind(false, kind);
        make_depositor_sender(&mut wallet, &store);
        wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
        let receipt = client
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
        let sent = wallet.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to, wallet.htlc.address, "{kind:?}");
        let signature = match kind {
            EvmHtlcKind::Erc20 => "refund(bytes32,uint256,address,address,uint256)",
            EvmHtlcKind::Native => "refund(bytes32,uint256,address,uint256)",
        };
        assert_eq!(sent[0].data[..4], selector(signature), "{kind:?}");
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
}

fn collab_params(wallet: &Wallet, path: &str) -> Value {
    let h = &wallet.htlc;
    json!({
        "coordinator_address": wallet.coordinator,
        "server_signer_address": h.claimer,
        "preimage_hash": h.hash_lock,
        "amount": h.amount.to_string(),
        "token": h.token,
        "claim_address": h.claimer,
        "timelock": h.timelock,
        "chain_id": h.chain_id,
        "sweep_token": h.token,
        "min_amount_out": "0",
        "calls_hash": B256::ZERO,
        "coordinator_version": 5,
        "htlc_address": h.address,
        "htlc_version": if h.token.is_zero() { 2 } else { 7 },
        "htlc_kind": if h.token.is_zero() { "native" } else { "erc20" },
        "refund_path": path,
    })
}

fn collab_signature(wallet: &Wallet) -> Value {
    let h = &wallet.htlc;
    json!({
        "id": "test",
        "htlc_address": h.address,
        "htlc_version": if h.token.is_zero() { 2 } else { 7 },
        "htlc_kind": if h.token.is_zero() { "native" } else { "erc20" },
        "chain_id": h.chain_id,
        "preimage_hash": h.hash_lock,
        "amount": h.amount.to_string(),
        "token": h.token,
        "sender": h.sender,
        "claim_address": h.claimer,
        "timelock": h.timelock,
        "v": 27,
        "r": B256::repeat_byte(0x11),
        "s": B256::repeat_byte(0x22),
    })
}

#[tokio::test]
async fn collab_refund_sends_refund_by_sig_from_the_depositor() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    for kind in [EvmHtlcKind::Erc20, EvmHtlcKind::Native] {
        let (_, mut wallet, store) = setup_kind(false, kind);
        make_depositor_sender(&mut wallet, &store);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/swap/test/collab-refund-evm/params"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(collab_params(&wallet, "direct")),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/swap/test/collab-refund-evm/signature"))
            .respond_with(ResponseTemplate::new(200).set_body_json(collab_signature(&wallet)))
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .build()
            .unwrap();
        // Before the timelock: the signature, not the clock, unlocks it.
        let receipt = client
            .collab_refund_evm_with_wallet(
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
        let sent = wallet.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to, wallet.htlc.address);
        let (signature, words) = match kind {
            EvmHtlcKind::Erc20 => (
                "refundBySig(bytes32,uint256,address,uint256,uint8,bytes32,bytes32)",
                7,
            ),
            EvmHtlcKind::Native => (
                "refundBySig(bytes32,uint256,uint256,uint8,bytes32,bytes32)",
                6,
            ),
        };
        assert_eq!(sent[0].data[..4], selector(signature), "{kind:?}");
        assert_eq!(sent[0].data.len(), 4 + words * 32);
        assert!(sent[0].data.ends_with(B256::repeat_byte(0x22).as_slice()));
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
}

#[tokio::test]
async fn collab_refund_refuses_other_paths_and_other_terms() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let options = |wallet: &Wallet| RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    // A coordinator-sender deployment: not this SDK's flow, nothing is sent.
    {
        let (_, wallet, store) = setup(false);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/swap/test/collab-refund-evm/params"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(collab_params(&wallet, "coordinator")),
            )
            .mount(&server)
            .await;
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(store)
            .build()
            .unwrap();
        let error = client
            .collab_refund_evm_with_wallet("test", &wallet, options(&wallet))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("coordinator"), "{error}");
        assert!(wallet.sent.lock().unwrap().is_empty());
    }
    // A signature over other terms than this wallet's lock is refused unsent.
    {
        let (_, mut wallet, store) = setup(false);
        make_depositor_sender(&mut wallet, &store);
        let mut signature = collab_signature(&wallet);
        signature["timelock"] = json!(wallet.htlc.timelock + 1);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/swap/test/collab-refund-evm/params"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(collab_params(&wallet, "direct")),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/swap/test/collab-refund-evm/signature"))
            .respond_with(ResponseTemplate::new(200).set_body_json(signature))
            .mount(&server)
            .await;
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .build()
            .unwrap();
        let error = client
            .collab_refund_evm_with_wallet("test", &wallet, options(&wallet))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("other terms"), "{error}");
        assert!(wallet.sent.lock().unwrap().is_empty());
        assert_eq!(
            store
                .get_swap("test")
                .unwrap()
                .unwrap()
                .metadata
                .evm_refund_started,
            None
        );
    }
}

#[tokio::test]
async fn depositor_sender_locks_are_readable_by_any_wallet_but_refunded_only_by_the_owner() {
    let (client, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    let reader = Submitter {
        wallet: &wallet,
        depositor: wallet.owner,
    };
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    // The Arkade claim's shortfall check reads the lock through any reader.
    let htlc = client
        .read_funded_evm_htlc("test", &reader, &options, true, true)
        .await
        .unwrap();
    assert_eq!(htlc.sender, wallet.owner);
    // The HTLC's refund pays msg.sender, so another wallet cannot send it.
    wallet.now.store(wallet.htlc.timelock, Ordering::Relaxed);
    let error = client
        .refund_evm_with_wallet("test", &reader, options)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("does not match swap"), "{error}");
    assert!(wallet.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_recorded_collab_refund_is_verified_without_the_api() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::path_regex;
    let (_, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    let recorded = B256::repeat_byte(77);
    let mut stored = store.get_swap("test").unwrap().unwrap();
    stored.metadata.evm_refund_started = Some(true);
    stored.metadata.evm_refund_txid = Some(recorded.to_string());
    store.store_swap(stored).unwrap();
    let server = MockServer::start().await;
    Mock::given(path_regex("collab-refund-evm"))
        .respond_with(ResponseTemplate::new(503))
        .expect(0)
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store)
        .build()
        .unwrap();
    let receipt = client
        .collab_refund_evm_with_wallet(
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
    assert_eq!(receipt.transaction_hash, recorded);
    assert!(wallet.sent.lock().unwrap().is_empty());
}

fn rehash_params(wallet: &Wallet, new_timelock: u64) -> Value {
    let h = &wallet.htlc;
    json!({
        "id": "test",
        "htlc_address": h.address,
        "htlc_version": if h.token.is_zero() { 2 } else { 7 },
        "htlc_kind": if h.token.is_zero() { "native" } else { "erc20" },
        "chain_id": h.chain_id,
        "preimage_hash": h.hash_lock,
        "amount": h.amount.to_string(),
        "token": h.token,
        "sender": h.sender,
        "claim_address": h.claimer,
        "timelock": h.timelock,
        "new_timelock": new_timelock,
        // The fixture invoices are old and pay 1000 sats.
        "max_invoice_amount_sats": 20_000,
        "min_invoice_validity_secs": 0,
    })
}

fn rehash_signature(wallet: &Wallet, new_hash: B256, new_timelock: u64) -> Value {
    let mut value = rehash_params(wallet, new_timelock);
    value["new_preimage_hash"] = json!(new_hash);
    value["v"] = json!(28);
    value["r"] = json!(B256::repeat_byte(0x11));
    value["s"] = json!(B256::repeat_byte(0x22));
    value
}

async fn rehash_server(params: Value, signature: Value) -> wiremock::MockServer {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/swap/test/rehash-evm/params"))
        .respond_with(ResponseTemplate::new(200).set_body_json(params))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/swap/test/rehash-evm"))
        .respond_with(ResponseTemplate::new(200).set_body_json(signature))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn retry_with_invoice_moves_the_lock_and_the_refund_follows_it() {
    for kind in [EvmHtlcKind::Erc20, EvmHtlcKind::Native] {
        let (_, mut wallet, store) = setup_kind(false, kind);
        make_depositor_sender(&mut wallet, &store);
        let (invoice, payment_hash) = fixture_invoice("lnd_hold_22");
        let new_timelock = wallet.htlc.timelock + 7200;
        let server = rehash_server(
            rehash_params(&wallet, new_timelock),
            rehash_signature(&wallet, payment_hash, new_timelock),
        )
        .await;
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .build()
            .unwrap();
        let options = RefundOptions {
            htlc: wallet.htlc.address,
            lock_token: wallet.htlc.token,
            coordinator: Some(wallet.coordinator),
            timeout: Duration::from_secs(10),
        };

        let moved = client
            .retry_swap_with_invoice_with_wallet("test", &invoice, &wallet, options.clone())
            .await
            .unwrap();

        assert_eq!(moved.new_hash_lock, payment_hash);
        assert_eq!(moved.new_timelock, new_timelock);
        assert_eq!(wallet.signatures.load(Ordering::Relaxed), 1);
        {
            let sent = wallet.sent.lock().unwrap();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].to, wallet.htlc.address);
            let (signature, words) = match kind {
                EvmHtlcKind::Erc20 => (
                    "rehash(bytes32,uint256,address,uint256,bytes32,uint256,uint8,bytes32,bytes32)",
                    9,
                ),
                EvmHtlcKind::Native => (
                    "rehash(bytes32,uint256,uint256,bytes32,uint256,uint8,bytes32,bytes32)",
                    8,
                ),
            };
            assert_eq!(sent[0].data[..4], selector(signature), "{kind:?}");
            assert_eq!(sent[0].data.len(), 4 + words * 32);
        }
        let history = store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_rehashes
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].txid, moved.tx_hash.to_string());
        assert_eq!(history[0].from_hash_lock, wallet.htlc.hash_lock.to_string());
        assert_eq!(history[0].hash_lock, payment_hash.to_string());
        assert_eq!(history[0].timelock, new_timelock);

        // The replacement payment failed too; after the new timelock the owner
        // refunds the lock that exists, from the receipts alone.
        server.reset().await;
        wallet.now.store(new_timelock, Ordering::Relaxed);
        client
            .refund_evm_with_wallet("test", &wallet, options)
            .await
            .unwrap();
        let sent = wallet.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].to, wallet.htlc.address);
        assert!(
            sent[1].data[4..36] == payment_hash.as_slice()[..],
            "{kind:?}: the refund names the new hash"
        );
    }
}

#[tokio::test]
async fn a_rehash_signature_off_the_authorized_terms_is_refused_unsent() {
    let (_, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    let (invoice, payment_hash) = fixture_invoice("lnd_hold_22");
    let new_timelock = wallet.htlc.timelock + 7200;
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    for (signature, expected) in [
        (
            rehash_signature(&wallet, B256::repeat_byte(0x99), new_timelock),
            "another hash",
        ),
        (
            rehash_signature(&wallet, payment_hash, new_timelock + 7 * 24 * 3600),
            "later than the advertised",
        ),
    ] {
        let server = rehash_server(rehash_params(&wallet, new_timelock), signature).await;
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .build()
            .unwrap();
        let error = client
            .retry_swap_with_invoice_with_wallet("test", &invoice, &wallet, options.clone())
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    assert!(wallet.sent.lock().unwrap().is_empty());
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_rehashes,
        None
    );
}

#[tokio::test]
async fn a_reverted_rehash_leaves_no_record() {
    let (_, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    wallet
        .reverted_receipts
        .lock()
        .unwrap()
        .push(B256::repeat_byte(99));
    let (invoice, payment_hash) = fixture_invoice("lnd_hold_22");
    let new_timelock = wallet.htlc.timelock + 7200;
    let server = rehash_server(
        rehash_params(&wallet, new_timelock),
        rehash_signature(&wallet, payment_hash, new_timelock),
    )
    .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let error = client
        .retry_swap_with_invoice_with_wallet(
            "test",
            &invoice,
            &wallet,
            RefundOptions {
                htlc: wallet.htlc.address,
                lock_token: wallet.htlc.token,
                coordinator: Some(wallet.coordinator),
                timeout: Duration::from_secs(10),
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("reverted"), "{error}");
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_rehashes,
        Some(vec![])
    );
}

#[tokio::test]
async fn the_servers_snapshot_is_accepted_once_it_follows_a_verified_rehash() {
    use wiremock::Mock;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let (_, mut wallet, store) = setup_fixture("evm_to_lightning", false, EvmHtlcKind::Erc20);
    make_depositor_sender(&mut wallet, &store);
    let (new_invoice, new_hash) = fixture_invoice("lnd_hold_22");
    let (other_invoice, other_hash) = fixture_invoice("lnd_hold_33");
    let new_timelock = wallet.htlc.timelock + 7200;
    let server = rehash_server(
        rehash_params(&wallet, new_timelock),
        rehash_signature(&wallet, new_hash, new_timelock),
    )
    .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    client
        .retry_swap_with_invoice_with_wallet("test", &new_invoice, &wallet, options)
        .await
        .unwrap();

    // The server promoted the move: its copy now names the new invoice and hash.
    let stored = store.get_swap("test").unwrap().unwrap();
    let mut promoted = serde_json::to_value(&stored.response).unwrap();
    promoted["client_lightning_invoice"] = json!(new_invoice);
    promoted["hash_lock"] = json!(new_hash);
    promoted["evm_refund_locktime"] = json!(new_timelock);
    let mut unrelated = promoted.clone();
    unrelated["client_lightning_invoice"] = json!(other_invoice);
    unrelated["hash_lock"] = json!(other_hash);
    for (snapshot, accepted) in [(promoted, true), (unrelated, false)] {
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/swap/test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(snapshot))
            .mount(&server)
            .await;
        let result = client.get_swap_details("test").await;
        assert_eq!(result.is_ok(), accepted, "{result:?}");
    }
}

#[tokio::test]
async fn an_invoice_off_the_advertised_limits_is_refused_before_signing() {
    let (_, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    let (invoice, payment_hash) = fixture_invoice("lnd_hold_22");
    let new_timelock = wallet.htlc.timelock + 7200;
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    for (params, expected) in [
        (
            {
                let mut p = rehash_params(&wallet, new_timelock);
                p["max_invoice_amount_sats"] = json!(999);
                p
            },
            "more than the swap pays out",
        ),
        (
            {
                let mut p = rehash_params(&wallet, new_timelock);
                // The fixture invoice expires in 2027; ask for ten years.
                p["min_invoice_validity_secs"] = json!(315_360_000);
                p
            },
            "must stay valid",
        ),
    ] {
        let server = rehash_server(
            params,
            rehash_signature(&wallet, payment_hash, new_timelock),
        )
        .await;
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(store.clone())
            .build()
            .unwrap();
        let error = client
            .retry_swap_with_invoice_with_wallet("test", &invoice, &wallet, options.clone())
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    assert_eq!(wallet.signatures.load(Ordering::Relaxed), 0);
    assert!(wallet.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_rehash_whose_broadcast_response_was_lost_blocks_until_attached() {
    let (_, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    wallet.fail_send.store(true, Ordering::Relaxed);
    let (invoice, payment_hash) = fixture_invoice("lnd_hold_22");
    let new_timelock = wallet.htlc.timelock + 7200;
    let server = rehash_server(
        rehash_params(&wallet, new_timelock),
        rehash_signature(&wallet, payment_hash, new_timelock),
    )
    .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    let error = client
        .retry_swap_with_invoice_with_wallet("test", &invoice, &wallet, options.clone())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Transport(_)), "{error}");
    // The tx may have reached the network: the terms are journaled, hash-less.
    let history = store
        .get_swap("test")
        .unwrap()
        .unwrap()
        .metadata
        .evm_rehashes
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].txid, "");
    assert_eq!(history[0].hash_lock, payment_hash.to_string());

    // Nothing runs on the lock until the submission is resolved.
    wallet.fail_send.store(false, Ordering::Relaxed);
    wallet.now.store(new_timelock, Ordering::Relaxed);
    let error = client
        .refund_evm_with_wallet("test", &wallet, options.clone())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unresolved"), "{error}");
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);

    // The operator finds the mined rehash and attaches it: the mock's receipt
    // for any hash follows the last sent call, the rehash.
    client
        .attach_evm_rehash_transaction("test", B256::repeat_byte(99))
        .await
        .unwrap();
    client
        .refund_evm_with_wallet("test", &wallet, options)
        .await
        .unwrap();
    let sent = wallet.sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].data[4..36], payment_hash.as_slice()[..]);
}

#[tokio::test]
async fn a_released_rehash_submission_leaves_the_lock_where_it_was() {
    let (client, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    let mut stored = store.get_swap("test").unwrap().unwrap();
    stored.metadata.evm_rehashes = Some(vec![crate::EvmRehashRecord {
        txid: String::new(),
        from_hash_lock: wallet.htlc.hash_lock.to_string(),
        from_timelock: wallet.htlc.timelock,
        hash_lock: B256::repeat_byte(0x22).to_string(),
        timelock: wallet.htlc.timelock + 7200,
        amount: wallet.htlc.amount.to_string(),
        token: wallet.htlc.token.to_string(),
    }]);
    store.store_swap(stored).unwrap();
    assert!(
        client
            .attach_evm_rehash_transaction("test", B256::ZERO)
            .await
            .is_err()
    );
    client.release_evm_rehash_submission("test").await.unwrap();
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_rehashes,
        Some(vec![])
    );
    assert!(client.release_evm_rehash_submission("test").await.is_err());
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
    let sent = wallet.sent.lock().unwrap();
    assert_eq!(sent[0].data[4..36], wallet.htlc.hash_lock.as_slice()[..]);
}

#[tokio::test]
async fn a_rehash_that_never_mines_names_its_remedy_and_a_replacement_can_be_attached() {
    let (_, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    let (invoice, payment_hash) = fixture_invoice("lnd_hold_22");
    let new_timelock = wallet.htlc.timelock + 7200;
    // The broadcast returns a hash the node then never serves a receipt for.
    let dropped = B256::repeat_byte(0xd0);
    wallet.next_hashes.lock().unwrap().push(dropped);
    wallet.missing_receipts.lock().unwrap().push(dropped);
    let server = rehash_server(
        rehash_params(&wallet, new_timelock),
        rehash_signature(&wallet, payment_hash, new_timelock),
    )
    .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    let error = client
        .retry_swap_with_invoice_with_wallet("test", &invoice, &wallet, options.clone())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("has not confirmed"), "{error}");
    assert!(
        error.to_string().contains("attach_evm_rehash_transaction"),
        "{error}"
    );
    wallet.now.store(new_timelock, Ordering::Relaxed);
    let error = client
        .refund_evm_with_wallet("test", &wallet, options.clone())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("has not confirmed"), "{error}");

    // The wallet sped it up: the replacement mined (the mock answers any hash
    // with the last sent call's event) and is attached over the dropped hash.
    client
        .attach_evm_rehash_transaction("test", B256::repeat_byte(0xd1))
        .await
        .unwrap();
    client
        .refund_evm_with_wallet("test", &wallet, options)
        .await
        .unwrap();
    let sent = wallet.sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].data[4..36], payment_hash.as_slice()[..]);
}

#[tokio::test]
async fn a_repeated_retry_with_the_same_invoice_returns_the_recorded_rehash() {
    let (_, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    let (invoice, payment_hash) = fixture_invoice("lnd_hold_22");
    let new_timelock = wallet.htlc.timelock + 7200;
    let server = rehash_server(
        rehash_params(&wallet, new_timelock),
        rehash_signature(&wallet, payment_hash, new_timelock),
    )
    .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    let first = client
        .retry_swap_with_invoice_with_wallet("test", &invoice, &wallet, options.clone())
        .await
        .unwrap();

    // The caller's first call timed out on its side and it calls again, with
    // the server down and the lock already moved: the recorded move is
    // verified from its receipt and returned, nothing is signed or sent.
    server.reset().await;
    wallet.settled.store(true, Ordering::Relaxed);
    let again = client
        .retry_swap_with_invoice_with_wallet("test", &invoice, &wallet, options)
        .await
        .unwrap();
    assert_eq!(again, first);
    assert_eq!(wallet.signatures.load(Ordering::Relaxed), 1);
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn the_depositors_collab_refund_resends_an_unrecorded_submission_and_checks_gas() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let (_, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    let mut stored = store.get_swap("test").unwrap().unwrap();
    stored.metadata.evm_refund_started = Some(true);
    store.store_swap(stored).unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/swap/test/collab-refund-evm/params"))
        .respond_with(ResponseTemplate::new(200).set_body_json(collab_params(&wallet, "direct")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/swap/test/collab-refund-evm/signature"))
        .respond_with(ResponseTemplate::new(200).set_body_json(collab_signature(&wallet)))
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    // Not the depositor path: the reservation without a hash blocks.
    let error = client
        .collab_refund_evm("test", &wallet, options.clone(), false)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unresolved"), "{error}");
    // The depositor path refuses before reserving when it cannot pay the gas…
    wallet.native_wei.store(0, Ordering::Relaxed);
    let error = client
        .collab_refund_evm("test", &wallet, options.clone(), true)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InsufficientGas(_)), "{error}");
    assert!(wallet.sent.lock().unwrap().is_empty());
    // …and resends while the lock is active once it can.
    wallet.native_wei.store(1_000_000, Ordering::Relaxed);
    let receipt = client
        .collab_refund_evm("test", &wallet, options, true)
        .await
        .unwrap();
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
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
async fn a_cancelled_rehash_frees_the_invoice_for_a_fresh_attempt() {
    let (_, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    wallet.fail_send.store(true, Ordering::Relaxed);
    let (invoice, payment_hash) = fixture_invoice("lnd_hold_22");
    let new_timelock = wallet.htlc.timelock + 7200;
    let server = rehash_server(
        rehash_params(&wallet, new_timelock),
        rehash_signature(&wallet, payment_hash, new_timelock),
    )
    .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    assert!(
        client
            .retry_swap_with_invoice_with_wallet("test", &invoice, &wallet, options.clone())
            .await
            .is_err()
    );
    // The operator cancelled the stuck rehash: the mined replacement carries
    // no SwapRehashed (the mock derives the event from the last sent call,
    // which is now a plain transfer).
    wallet.fail_send.store(false, Ordering::Relaxed);
    wallet.sent.lock().unwrap().push(EvmTransaction {
        chain_id: wallet.chain_id(),
        from: wallet.owner,
        to: wallet.owner,
        value: U256::ZERO,
        data: Bytes::new(),
    });
    client
        .attach_evm_rehash_transaction("test", B256::repeat_byte(0xca))
        .await
        .unwrap();

    // A repeat with the same invoice forgets the record and moves the lock.
    let moved = client
        .retry_swap_with_invoice_with_wallet("test", &invoice, &wallet, options)
        .await
        .unwrap();
    assert_eq!(moved.new_hash_lock, payment_hash);
    let history = store
        .get_swap("test")
        .unwrap()
        .unwrap()
        .metadata
        .evm_rehashes
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].txid, moved.tx_hash.to_string());
}

#[tokio::test]
async fn a_superseded_rehash_is_not_reported_as_the_current_lock() {
    let (client, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    let first = B256::repeat_byte(0xa1);
    let mut stored = store.get_swap("test").unwrap().unwrap();
    stored.metadata.evm_rehashes = Some(vec![
        crate::EvmRehashRecord {
            txid: B256::repeat_byte(0xa0).to_string(),
            from_hash_lock: wallet.htlc.hash_lock.to_string(),
            from_timelock: wallet.htlc.timelock,
            hash_lock: first.to_string(),
            timelock: wallet.htlc.timelock + 7200,
            amount: wallet.htlc.amount.to_string(),
            token: wallet.htlc.token.to_string(),
        },
        crate::EvmRehashRecord {
            txid: B256::repeat_byte(0xb0).to_string(),
            from_hash_lock: first.to_string(),
            from_timelock: wallet.htlc.timelock + 7200,
            hash_lock: B256::repeat_byte(0xb1).to_string(),
            timelock: wallet.htlc.timelock + 14_400,
            amount: wallet.htlc.amount.to_string(),
            token: wallet.htlc.token.to_string(),
        },
    ]);
    store.store_swap(stored).unwrap();
    // The first move is on record but no longer where the lock is.
    assert!(
        client
            .recorded_rehash("test", &wallet, first)
            .await
            .unwrap()
            .is_none()
    );
    assert!(wallet.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_recorded_rehash_under_other_terms_is_refused_not_trusted() {
    let (client, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    let (_, payment_hash) = fixture_invoice("lnd_hold_22");
    let new_timelock = wallet.htlc.timelock + 7200;
    // The mock's receipt for the record's tx carries the SwapRehashed of the
    // last sent call: a rehash of this lock to the invoice under `new_timelock`.
    wallet.sent.lock().unwrap().push(transaction(
        &wallet,
        wallet.htlc.address,
        Htlc::rehashCall {
            preimageHash: wallet.htlc.hash_lock,
            amount: wallet.htlc.amount,
            token: wallet.htlc.token,
            timelock: U256::from(wallet.htlc.timelock),
            newPreimageHash: payment_hash,
            newTimelock: U256::from(new_timelock),
            v: 28,
            r: B256::repeat_byte(0x11),
            s: B256::repeat_byte(0x22),
        }
        .abi_encode(),
    ));
    let record = |timelock: u64| crate::EvmRehashRecord {
        txid: B256::repeat_byte(0xe0).to_string(),
        from_hash_lock: wallet.htlc.hash_lock.to_string(),
        from_timelock: wallet.htlc.timelock,
        hash_lock: payment_hash.to_string(),
        timelock,
        amount: wallet.htlc.amount.to_string(),
        token: wallet.htlc.token.to_string(),
    };
    // A record claiming another destination timelock than the event's.
    let mut stored = store.get_swap("test").unwrap().unwrap();
    stored.metadata.evm_rehashes = Some(vec![record(new_timelock + 1)]);
    store.store_swap(stored).unwrap();
    let error = client
        .recorded_rehash("test", &wallet, payment_hash)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("other terms"), "{error}");
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_rehashes
            .map(|h| h.len()),
        Some(1),
        "the record is kept for the operator to fix"
    );
    // The matching record is verified and returned.
    let mut stored = store.get_swap("test").unwrap().unwrap();
    stored.metadata.evm_rehashes = Some(vec![record(new_timelock)]);
    store.store_swap(stored).unwrap();
    let found = client
        .recorded_rehash("test", &wallet, payment_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.new_timelock, new_timelock);
}

/// How the server answers `collab-refund-evm/params` in a recovery test.
enum Terms {
    Amount(U256),
    Malformed,
    Stalled,
    Down,
}

/// A HTLCErc20 v7 / HTLCNative v2 EVM to Lightning swap whose lock a retry
/// moved to a new invoice, recovered from the seed into an empty store once
/// the server promoted the move: the server names the new hash and the rehash
/// tx as the funding, and nothing local records the move. `locked` is the
/// amount on chain, `terms` the server's answer for it, and `edit` changes
/// the server's snapshot. The returned wallet holds the moved lock.
async fn recovered_after_an_unrecorded_rehash(
    kind: EvmHtlcKind,
    locked: U256,
    terms: Terms,
    edit: impl FnOnce(&mut Value),
) -> (Client, Wallet, wiremock::MockServer) {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    let (_, mut wallet, store) = setup_fixture("evm_to_lightning", false, kind);
    make_depositor_sender(&mut wallet, &store);
    wallet.htlc.amount = locked;
    let (invoice, new_hash) = fixture_invoice("lnd_hold_22");
    let new_timelock = wallet.htlc.timelock + 7200;
    let rehash_tx = B256::repeat_byte(0xf0);
    let mut snapshot =
        serde_json::to_value(&store.get_swap("test").unwrap().unwrap().response).unwrap();
    snapshot["client_lightning_invoice"] = json!(invoice);
    snapshot["hash_lock"] = json!(new_hash);
    snapshot["evm_refund_locktime"] = json!(new_timelock);
    snapshot["evm_fund_txid"] = json!(rehash_tx);
    snapshot["evm_expected_sats"] = json!("100000");
    edit(&mut snapshot);
    // The chain: the rehash tx moved the funded lock to the new terms.
    let moved = EvmHtlc {
        hash_lock: new_hash,
        timelock: new_timelock,
        ..wallet.htlc.clone()
    };
    wallet.fixed_receipts.lock().unwrap().insert(
        rehash_tx,
        vec![event_log(
            SwapRehashed {
                preimageHash: wallet.htlc.hash_lock,
                key: wallet.htlc.key(),
                newKey: moved.key(),
                newPreimageHash: new_hash,
            },
            wallet.htlc.address,
        )],
    );
    wallet.htlc = moved;
    wallet.now.store(new_timelock, Ordering::Relaxed);
    let server = MockServer::start().await;
    let mut recovered = snapshot.clone();
    recovered["index"] = json!(0);
    Mock::given(method("POST"))
        .and(path("/swap/recover"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "swaps": [recovered],
            "highest_index": 0,
            "next_index": 1,
            "scanned_until": 0,
            "complete": true,
        })))
        .mount(&server)
        .await;
    let params = |amount: String| crate::evm::CollabRefundEvmParams {
        htlc_address: wallet.htlc.address.to_string(),
        htlc_version: if kind == EvmHtlcKind::Native { 2 } else { 7 },
        htlc_kind: kind,
        chain_id: wallet.htlc.chain_id,
        preimage_hash: snapshot["hash_lock"].as_str().unwrap().to_owned(),
        amount,
        token: wallet.htlc.token.to_string(),
        claim_address: wallet.htlc.claimer.to_string(),
        timelock: snapshot["evm_refund_locktime"].as_u64().unwrap(),
        refund_path: "direct".into(),
    };
    let response = match terms {
        Terms::Amount(amount) => {
            Some(ResponseTemplate::new(200).set_body_json(params(amount.to_string())))
        }
        Terms::Malformed => Some(ResponseTemplate::new(200).set_body_json(params("lots".into()))),
        Terms::Stalled => Some(
            ResponseTemplate::new(200)
                .set_body_json(params(locked.to_string()))
                .set_delay(Duration::from_secs(600)),
        ),
        Terms::Down => None,
    };
    if let Some(response) = response {
        Mock::given(method("GET"))
            .and(path("/api/swap/test/collab-refund-evm/params"))
            .respond_with(response)
            .mount(&server)
            .await;
    }
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(Arc::new(InMemorySwapStorage::new()))
        .build()
        .unwrap();
    client.recover_swaps(None).await.unwrap();
    let stored = client.get_stored_swap("test").unwrap().unwrap();
    assert!(stored.metadata.evm_rehashes.is_none());
    assert!(stored.metadata.evm_fund_txid.is_none());
    assert!(stored.metadata.evm_recovered_lock.is_none());
    (client, wallet, server)
}

fn refund_options(wallet: &Wallet) -> RefundOptions {
    RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(5),
    }
}

async fn refund_recovered(client: &Client, wallet: &Wallet) -> Result<EvmReceipt> {
    client
        .refund_evm_with_wallet("test", wallet, refund_options(wallet))
        .await
}

/// The last sent tx as an HTLC refund: its hash lock, amount and timelock.
fn sent_refund(wallet: &Wallet) -> (B256, U256, U256) {
    let sent = wallet.sent.lock().unwrap();
    let refund = sent.last().unwrap();
    assert_eq!(
        refund.to, wallet.htlc.address,
        "refunded on the HTLC itself"
    );
    Htlc::refundCall::abi_decode(&refund.data)
        .map(|call| (call.preimageHash, call.amount, call.timelock))
        .or_else(|_| {
            super::refund::NativeHtlc::refundCall::abi_decode(&refund.data)
                .map(|call| (call.preimageHash, call.amount, call.timelock))
        })
        .unwrap()
}

#[tokio::test]
async fn a_rehash_recovered_from_the_seed_refunds_the_moved_lock_from_empty_storage() {
    for kind in [EvmHtlcKind::Erc20, EvmHtlcKind::Native] {
        // The locked amount differs from the quote; only the server knows it.
        let (client, wallet, _server) = recovered_after_an_unrecorded_rehash(
            kind,
            U256::from(100_123),
            Terms::Amount(U256::from(100_123)),
            |_| {},
        )
        .await;
        refund_recovered(&client, &wallet)
            .await
            .unwrap_or_else(|error| panic!("{kind:?}: {error}"));
        assert_eq!(wallet.sent.lock().unwrap().len(), 1);
        assert_eq!(
            sent_refund(&wallet),
            (
                wallet.htlc.hash_lock,
                U256::from(100_123),
                U256::from(wallet.htlc.timelock)
            )
        );
        let metadata = client.get_stored_swap("test").unwrap().unwrap().metadata;
        assert_eq!(
            metadata.evm_fund_txid,
            Some(B256::repeat_byte(0xf0).to_string())
        );
        let saved = metadata.evm_recovered_lock.unwrap();
        assert_eq!(
            (saved.hash_lock, saved.amount, saved.sender),
            (
                wallet.htlc.hash_lock.to_string(),
                "100123".into(),
                wallet.owner.to_string()
            )
        );
    }
}

#[tokio::test]
async fn a_submitted_refund_of_a_recovered_lock_resumes_without_the_server() {
    let (client, wallet, server) = recovered_after_an_unrecorded_rehash(
        EvmHtlcKind::Erc20,
        U256::from(100_123),
        Terms::Amount(U256::from(100_123)),
        |_| {},
    )
    .await;
    // The refund is broadcast, but its receipt is not served yet.
    wallet
        .missing_receipts
        .lock()
        .unwrap()
        .push(B256::repeat_byte(99));
    refund_recovered(&client, &wallet).await.unwrap_err();
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
    // The terms endpoint goes down; the quote does not match the lock.
    server.reset().await;
    wallet.missing_receipts.lock().unwrap().clear();
    refund_recovered(&client, &wallet).await.unwrap();
    assert_eq!(wallet.sent.lock().unwrap().len(), 1, "verified, not resent");
}

#[tokio::test]
async fn a_recovered_lock_follows_a_later_retry_to_the_refund() {
    use wiremock::Mock;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    for kind in [EvmHtlcKind::Erc20, EvmHtlcKind::Native] {
        let (client, mut wallet, server) = recovered_after_an_unrecorded_rehash(
            kind,
            U256::from(100_123),
            Terms::Amount(U256::from(100_123)),
            |_| {},
        )
        .await;
        // The payment to the recovered invoice fails too; the user retries
        // with another one, and the lock moves again.
        let (invoice, payment_hash) = fixture_invoice("lnd_hold_33");
        let new_timelock = wallet.htlc.timelock + 7200;
        Mock::given(method("GET"))
            .and(path("/api/swap/test/rehash-evm/params"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(rehash_params(&wallet, new_timelock)),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/swap/test/rehash-evm"))
            .respond_with(ResponseTemplate::new(200).set_body_json(rehash_signature(
                &wallet,
                payment_hash,
                new_timelock,
            )))
            .mount(&server)
            .await;
        wallet
            .next_hashes
            .lock()
            .unwrap()
            .push(B256::repeat_byte(0xf1));
        wallet
            .now
            .store(wallet.htlc.timelock - 100, Ordering::Relaxed);
        client
            .retry_swap_with_invoice_with_wallet("test", &invoice, &wallet, refund_options(&wallet))
            .await
            .unwrap_or_else(|error| panic!("{kind:?}: {error}"));
        // The server has not promoted the second move, and its terms are gone.
        server.reset().await;
        wallet.htlc.hash_lock = payment_hash;
        wallet.htlc.timelock = new_timelock;
        wallet.now.store(new_timelock, Ordering::Relaxed);
        refund_recovered(&client, &wallet)
            .await
            .unwrap_or_else(|error| panic!("{kind:?}: {error}"));
        assert_eq!(wallet.sent.lock().unwrap().len(), 2);
        assert_eq!(
            sent_refund(&wallet),
            (payment_hash, U256::from(100_123), U256::from(new_timelock))
        );
    }
}

#[tokio::test]
async fn a_matching_quote_is_used_without_asking_the_server() {
    // A stalled terms endpoint would outlast the refund's timeout.
    let (client, wallet, server) = recovered_after_an_unrecorded_rehash(
        EvmHtlcKind::Erc20,
        U256::from(100_000),
        Terms::Stalled,
        |_| {},
    )
    .await;
    refund_recovered(&client, &wallet).await.unwrap();
    assert_eq!(wallet.sent.lock().unwrap().len(), 1);
    let asked = server.received_requests().await.unwrap();
    assert!(
        asked
            .iter()
            .all(|request| !request.url.path().ends_with("collab-refund-evm/params")),
        "the terms endpoint was asked"
    );
}

#[tokio::test]
async fn server_terms_that_do_not_hash_to_the_rehashed_lock_refund_nothing() {
    // A wrong amount, from both the terms and the quote; then terms that
    // are not an amount at all.
    for terms in [
        Terms::Amount(U256::from(99_999)),
        Terms::Malformed,
        Terms::Down,
    ] {
        let (client, wallet, _server) = recovered_after_an_unrecorded_rehash(
            EvmHtlcKind::Erc20,
            U256::from(100_123),
            terms,
            |_| {},
        )
        .await;
        let error = refund_recovered(&client, &wallet).await.unwrap_err();
        assert!(
            error.to_string().contains("do not match the lock"),
            "{error}"
        );
        assert!(wallet.sent.lock().unwrap().is_empty());
        let metadata = client.get_stored_swap("test").unwrap().unwrap().metadata;
        assert!(metadata.evm_recovered_lock.is_none());
    }
    // A timelock earlier than the one the rehash set, to refund early.
    let (client, wallet, _server) = recovered_after_an_unrecorded_rehash(
        EvmHtlcKind::Erc20,
        U256::from(100_000),
        Terms::Amount(U256::from(100_000)),
        |snapshot| {
            let timelock = snapshot["evm_refund_locktime"].as_u64().unwrap();
            snapshot["evm_refund_locktime"] = json!(timelock - 3600);
        },
    )
    .await;
    let error = refund_recovered(&client, &wallet).await.unwrap_err();
    assert!(
        error.to_string().contains("do not match the lock"),
        "{error}"
    );
    assert!(wallet.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_rehashed_lock_that_is_no_longer_active_refunds_nothing() {
    let (client, wallet, _server) = recovered_after_an_unrecorded_rehash(
        EvmHtlcKind::Erc20,
        U256::from(100_000),
        Terms::Amount(U256::from(100_000)),
        |_| {},
    )
    .await;
    wallet.settled.store(true, Ordering::Relaxed);
    let error = refund_recovered(&client, &wallet).await.unwrap_err();
    assert!(
        error.to_string().contains("rehashed is no longer active"),
        "{error}"
    );
    assert!(wallet.sent.lock().unwrap().is_empty());
    let metadata = client.get_stored_swap("test").unwrap().unwrap().metadata;
    assert!(
        metadata.evm_recovered_lock.is_none(),
        "unverified terms are not saved"
    );
}

#[test]
fn a_saved_recovered_lock_is_never_replaced() {
    let lock = |amount: &str| crate::storage::EvmRecoveredLock {
        txid: B256::repeat_byte(0xf0).to_string(),
        hash_lock: B256::repeat_byte(1).to_string(),
        timelock: 1,
        amount: amount.into(),
        token: Address::ZERO.to_string(),
        sender: Address::ZERO.to_string(),
        claim_address: Address::ZERO.to_string(),
    };
    let store = InMemorySwapStorage::new();
    let (_, _, setup_store) = setup(false);
    store
        .store_swap(setup_store.get_swap("test").unwrap().unwrap())
        .unwrap();
    for amount in ["1", "2"] {
        store
            .update_metadata(
                "test",
                SwapMetadata {
                    evm_recovered_lock: Some(lock(amount)),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_recovered_lock,
        Some(lock("1"))
    );
}

#[tokio::test]
async fn the_depositors_retry_refuses_without_gas_before_journaling_anything() {
    let (_, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    wallet.native_wei.store(0, Ordering::Relaxed);
    let (invoice, payment_hash) = fixture_invoice("lnd_hold_22");
    let new_timelock = wallet.htlc.timelock + 7200;
    let server = rehash_server(
        rehash_params(&wallet, new_timelock),
        rehash_signature(&wallet, payment_hash, new_timelock),
    )
    .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(store.clone())
        .build()
        .unwrap();
    let options = RefundOptions {
        htlc: wallet.htlc.address,
        lock_token: wallet.htlc.token,
        coordinator: Some(wallet.coordinator),
        timeout: Duration::from_secs(10),
    };
    let error = client
        .retry_swap_with_invoice_evm("test", &invoice, &wallet, options.clone(), true)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InsufficientGas(_)), "{error}");
    assert!(wallet.sent.lock().unwrap().is_empty());
    assert_eq!(
        store
            .get_swap("test")
            .unwrap()
            .unwrap()
            .metadata
            .evm_rehashes,
        None
    );
    // With gas, the same call goes through; nothing was left to reconcile.
    wallet.native_wei.store(1_000_000, Ordering::Relaxed);
    client
        .retry_swap_with_invoice_evm("test", &invoice, &wallet, options, true)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_record_without_the_locks_amount_falls_through_to_the_normal_flow() {
    let (client, mut wallet, store) = setup(false);
    make_depositor_sender(&mut wallet, &store);
    let (_, payment_hash) = fixture_invoice("lnd_hold_22");
    let mut stored = store.get_swap("test").unwrap().unwrap();
    stored.metadata.evm_rehashes = Some(vec![crate::EvmRehashRecord {
        txid: B256::repeat_byte(0xe0).to_string(),
        from_hash_lock: wallet.htlc.hash_lock.to_string(),
        from_timelock: wallet.htlc.timelock,
        hash_lock: payment_hash.to_string(),
        timelock: wallet.htlc.timelock + 7200,
        amount: String::new(),
        token: String::new(),
    }]);
    store.store_swap(stored).unwrap();
    assert!(
        client
            .recorded_rehash("test", &wallet, payment_hash)
            .await
            .unwrap()
            .is_none()
    );
}

/// A chain RPC serving `log` as the only HTLC event, mined at block 99 of
/// 100 with the tip at `timestamp`.
async fn chain_with_log(log: EvmLog, timestamp: u64) -> wiremock::MockServer {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(move |request: &wiremock::Request| {
        let body: Value = request.body_json().unwrap();
        let result = match body["method"].as_str().unwrap() {
            "eth_chainId" => json!("0x89"),
            "eth_getBlockByNumber" => {
                let number = if body["params"][0] == "latest" { "0x64" } else { body["params"][0].as_str().unwrap() };
                json!({"hash":B256::repeat_byte(9),"parentHash":B256::ZERO,"sha3Uncles":B256::ZERO,"miner":Address::ZERO,"stateRoot":B256::ZERO,"transactionsRoot":B256::ZERO,"receiptsRoot":B256::ZERO,"logsBloom":format!("0x{}","00".repeat(256)),"difficulty":"0x0","number":number,"gasLimit":"0x1000000","gasUsed":"0x0","timestamp":format!("{timestamp:#x}"),"extraData":"0x","mixHash":B256::ZERO,"nonce":"0x0000000000000000","transactions":[],"uncles":[]})
            }
            "eth_getLogs" => json!([{"address":log.address,"topics":log.topics,"data":format!("0x{}",hex::encode(&log.data)),"blockNumber":"0x63","blockHash":B256::repeat_byte(9),"transactionHash":B256::repeat_byte(7),"transactionIndex":"0x0","logIndex":"0x0","removed":false}]),
            method => panic!("unexpected {method}"),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
    }).mount(&server).await;
    server
}

#[tokio::test]
async fn tracking_observes_the_client_lock_under_the_sender_of_its_generation() {
    use crate::actions::ActionInput;
    use crate::actions::ActionKind;
    use crate::actions::ActionPolicy;
    use crate::actions::BridgeState;
    use crate::observation::ContractReader;
    use crate::observation::ContractRef;
    use crate::observation::EvmRpcReader;
    use crate::observation::HtlcState;
    use crate::observation::ReferenceOptions;
    for kind in [EvmHtlcKind::Erc20, EvmHtlcKind::Native] {
        for depositor_sender in [false, true] {
            let (client, mut wallet, store) = setup_fixture("evm_to_lightning", false, kind);
            if depositor_sender {
                make_depositor_sender(&mut wallet, &store);
            }
            let options = ReferenceOptions {
                evm_from_block: 1,
                ..ReferenceOptions::default()
            };
            let contracts = client.contract_references("test", &options).unwrap();
            let Some(ContractRef::Evm { htlc, .. }) = &contracts.client else {
                panic!("expected an EVM source reference")
            };
            // v6 / native v1 lock under the coordinator, v7 / native v2 under
            // the depositor.
            let sender = if depositor_sender {
                wallet.owner
            } else {
                wallet.coordinator
            };
            assert_eq!(htlc.sender, sender, "{kind:?} depositor={depositor_sender}");
            assert_eq!(*htlc, wallet.htlc);
            // An explicit sender still wins over either generation.
            let overridden = client
                .contract_references(
                    "test",
                    &ReferenceOptions {
                        evm_source_sender: Some(Address::repeat_byte(0x77)),
                        ..options.clone()
                    },
                )
                .unwrap();
            let Some(ContractRef::Evm { htlc: other, .. }) = overridden.client else {
                panic!("expected an EVM source reference")
            };
            assert_eq!(other.sender, Address::repeat_byte(0x77));
            // The lock as the chain records it, past its timelock.
            let h = &wallet.htlc;
            let chain = chain_with_log(
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
                ),
                h.timelock + 1,
            )
            .await;
            let reader = EvmRpcReader::connect(chain.uri().parse().unwrap(), 137, 1000)
                .await
                .unwrap();
            let observed = reader
                .observe(contracts.client.as_ref().unwrap())
                .await
                .unwrap();
            assert_eq!(observed.state, HtlcState::Confirmed);
            let actions = crate::actions::derive_actions(
                ActionInput {
                    contracts: &contracts,
                    client: Some(&observed),
                    server: None,
                    funding_submitted: false,
                    claim_submitted: false,
                    funding_uncertain: false,
                    submission_uncertain: false,
                    collaborative_refund_available: false,
                    bridge: &BridgeState::None,
                    server_status: None,
                },
                &ActionPolicy::default(),
            );
            assert!(
                actions
                    .actions
                    .iter()
                    .any(|action| action.kind == ActionKind::RefundUnilateral),
                "{actions:?}"
            );
        }
    }
}
