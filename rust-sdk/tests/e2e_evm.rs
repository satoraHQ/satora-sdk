//! Local-chain E2E tests against freshly built repository contracts and a fresh
//! Anvil per test. No fork RPC, private credentials, Docker or production funds.
//! API snapshots and the quote response are fixtures; no chain RPC is mocked.
//! Run `scripts/e2e/test-rust-sdk-local.sh` from the repository root.
#![cfg(feature = "gasless")]
#[path = "support/anvil.rs"]
mod anvil;
use alloy::primitives::keccak256;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolValue;
use anvil::*;
use lendaswap_sdk::StoredSwap;
use lendaswap_sdk::SwapMetadata;
use lendaswap_sdk::SwapStorage;
use lendaswap_sdk::aa::abi::Call;
use lendaswap_sdk::actions::ActionKind;
use lendaswap_sdk::actions::Outcome;
use lendaswap_sdk::evm::Address;
use lendaswap_sdk::evm::EvmClaimOptions;
use lendaswap_sdk::evm::FundingOptions;
use lendaswap_sdk::evm::RefundOptions;
use lendaswap_sdk::evm::RelayFundingOptions;
use lendaswap_sdk::evm::U256;
use lendaswap_sdk::observation::ContractManager;
use lendaswap_sdk::observation::ContractReader;
use lendaswap_sdk::observation::ContractRef;
use lendaswap_sdk::observation::EvmRpcReader;
use lendaswap_sdk::observation::HtlcState;
use lendaswap_sdk::observation::ManagerOptions;
use lendaswap_sdk::observation::SwapContracts;
use lendaswap_sdk::tracker::ActionExecutor;
use lendaswap_sdk::tracker::ExecutionFuture;
use lendaswap_sdk::tracker::ExecutionResult;
use lendaswap_sdk::tracker::SwapTracker;
use lendaswap_sdk::tracker::TrackerOptions;
use lendaswap_sdk::tracker::TrackingOptions;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::path;

fn reference(f: &Fixture, source: bool) -> ContractRef {
    ContractRef::Evm {
        htlc: lendaswap_sdk::evm::htlc::EvmHtlc {
            chain_id: CHAIN,
            address: f.htlc,
            hash_lock: f.hash,
            amount: U256::from(AMOUNT),
            token: f.token,
            sender: if source { f.coordinator } else { f.deployer },
            claimer: if source { f.deployer } else { f.owner },
            timelock: f.timelock,
        },
        from_block: 0,
        confirmations: 1,
    }
}
async fn reader(f: &Fixture) -> TestResult<EvmRpcReader> {
    Ok(EvmRpcReader::connect(f.url.clone(), CHAIN, 1000).await?)
}
alloy::sol! {
    interface NativeLock {
        function create(bytes32 preimageHash, address claimAddress, uint256 timelock) external payable;
    }
    struct NativeCall { address target; uint256 value; bytes callData; }
    interface NativeFunding {
        function executeAndCreate(NativeCall[] calls, bytes32 preimageHash, uint256 amount, address claimAddress, uint256 timelock) external payable;
    }
}
fn quantity(value: &serde_json::Value) -> TestResult<U256> {
    Ok(U256::from_str_radix(
        value
            .as_str()
            .ok_or("expected a hex quantity")?
            .trim_start_matches("0x"),
        16,
    )?)
}
async fn coin_balance(f: &Fixture, address: Address) -> TestResult<U256> {
    quantity(&f.rpc("eth_getBalance", json!([address, "latest"])).await?)
}

#[tokio::test]
#[ignore = "requires Anvil and freshly compiled repository contracts; run scripts/e2e/test-rust-sdk-local.sh"]
async fn sdk_claim_executes_signature_transfers_tokens_and_survives_restart() -> TestResult {
    let f = Fixture::new().await?;
    f.save_swap(false, None)?;
    let reader = reader(&f).await?;
    let reference = reference(&f, false);
    assert_eq!(reader.observe(&reference).await?.state, HtlcState::Absent);
    f.fund_target().await?;
    assert_eq!(
        reader.observe(&reference).await?.state,
        HtlcState::Confirmed
    );
    assert_eq!(f.balance(f.owner).await?, U256::ZERO);
    let receipt = f
        .client
        .claim_evm_with_wallet(ID, &f.wallet, f.claim_options(), vec![])
        .await?;
    assert!(receipt.success);
    assert_eq!(f.balance(f.owner).await?, U256::from(AMOUNT));
    assert_eq!(f.balance(f.htlc).await?, U256::ZERO);
    let observed = reader.observe(&reference).await?;
    assert_eq!(observed.state, HtlcState::SpentClaim);
    assert!(
        reference
            .hash_lock()
            .matches(observed.preimage.as_ref().unwrap().expose())
    );
    let reopened = f.reopen("http://127.0.0.1:1")?;
    assert_eq!(
        reopened
            .get_stored_swap(ID)?
            .unwrap()
            .metadata
            .evm_claim_txid,
        Some(receipt.transaction_hash.to_string())
    );
    assert!(
        reopened
            .claim_evm_with_wallet(ID, &f.wallet, f.claim_options(), vec![])
            .await
            .is_err()
    );
    assert_eq!(f.balance(f.owner).await?, U256::from(AMOUNT));
    Ok(())
}

async fn fund_source(f: &Fixture, api: &MockServer) -> TestResult<lendaswap_sdk::Client> {
    f.save_swap(true, None)?;
    f.transact(
        f.deployer,
        f.token,
        Token::transferCall {
            to: f.owner,
            amount: U256::from(AMOUNT),
        },
    )
    .await?;
    let calls: Vec<Call> = vec![];
    let quote = json!({
        "coordinator_address": f.coordinator,
        "permit2_address": lendaswap_sdk::aa::permit2::PERMIT2_ADDRESS,
        "source_token_address": f.token,
        "source_amount": AMOUNT.to_string(),
        "lock_token_address": f.token,
        "preimage_hash": f.hash,
        "claim_address": f.deployer,
        "timelock": f.timelock,
        "calls": [],
        "calls_hash": keccak256(calls.abi_encode())
    });
    Mock::given(path(format!("/swap/{ID}/swap-and-lock-calldata-permit2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(quote))
        .expect(2)
        .mount(api)
        .await;
    let client = f.reopen(&api.uri())?;
    let receipt = client
        .fund_swap_with_wallet(
            ID,
            &f.wallet,
            FundingOptions {
                coordinator: f.coordinator,
                htlc: f.htlc,
                lock_token: f.token,
                max_source_amount: U256::from(AMOUNT),
                allowed_call_targets: vec![],
                timeout: Duration::from_secs(20),
            },
        )
        .await?;
    assert!(receipt.success);
    assert_eq!(f.balance(f.owner).await?, U256::ZERO);
    assert_eq!(f.balance(f.htlc).await?, U256::from(AMOUNT));
    Ok(client)
}
#[tokio::test]
#[ignore = "requires Anvil and freshly compiled repository contracts; run scripts/e2e/test-rust-sdk-local.sh"]
async fn permit2_funding_and_timelocked_refund_resume_from_sqlite() -> TestResult {
    let f = Fixture::new().await?;
    let api = MockServer::start().await;
    let client = fund_source(&f, &api).await?;
    let options = RefundOptions {
        htlc: f.htlc,
        lock_token: f.token,
        coordinator: Some(f.coordinator),
        timeout: Duration::from_secs(20),
    };
    assert!(
        client
            .refund_evm_with_wallet(ID, &f.wallet, options.clone())
            .await
            .is_err()
    );
    assert_eq!(
        reader(&f).await?.observe(&reference(&f, true)).await?.state,
        HtlcState::Confirmed
    );
    f.rpc("evm_setNextBlockTimestamp", json!([f.timelock + 1]))
        .await?;
    f.rpc("evm_mine", json!([])).await?;
    let receipt = client
        .refund_evm_with_wallet(ID, &f.wallet, options.clone())
        .await?;
    assert_eq!(f.balance(f.owner).await?, U256::from(AMOUNT));
    assert_eq!(
        reader(&f).await?.observe(&reference(&f, true)).await?.state,
        HtlcState::SpentRefund
    );
    let reopened = f.reopen("http://127.0.0.1:1")?;
    let before = f
        .rpc("eth_getTransactionCount", json!([f.owner, "latest"]))
        .await?;
    assert_eq!(
        reopened
            .refund_evm_with_wallet(ID, &f.wallet, options)
            .await?
            .transaction_hash,
        receipt.transaction_hash
    );
    assert_eq!(
        f.rpc("eth_getTransactionCount", json!([f.owner, "latest"]))
            .await?,
        before
    );
    Ok(())
}
#[tokio::test]
#[ignore = "requires Anvil and freshly compiled repository contracts; run scripts/e2e/test-rust-sdk-local.sh"]
async fn canonical_observations_regress_after_anvil_reorg() -> TestResult {
    let f = Fixture::new().await?;
    f.save_swap(false, None)?;
    let reference = reference(&f, false);
    let reader = reader(&f).await?;
    let snapshot = f.rpc("evm_snapshot", json!([])).await?;
    f.fund_target().await?;
    assert_eq!(
        reader.observe(&reference).await?.state,
        HtlcState::Confirmed
    );
    f.client
        .claim_evm_with_wallet(ID, &f.wallet, f.claim_options(), vec![])
        .await?;
    assert_eq!(
        reader.observe(&reference).await?.state,
        HtlcState::SpentClaim
    );
    assert_eq!(f.rpc("evm_revert", json!([snapshot])).await?, json!(true));
    f.rpc("evm_mine", json!([])).await?;
    let observation = reader.observe(&reference).await?;
    assert_eq!(observation.state, HtlcState::Absent);
    assert!(observation.preimage.is_none());
    assert_eq!(f.balance(f.owner).await?, U256::ZERO);
    Ok(())
}
struct Claimer {
    client: lendaswap_sdk::Client,
    wallet: lendaswap_sdk::evm::RpcWallet,
    options: lendaswap_sdk::evm::EvmClaimOptions,
}
impl ActionExecutor for Claimer {
    fn execute<'a>(&'a self, id: &'a str, action: &'a ActionKind) -> ExecutionFuture<'a> {
        Box::pin(async move {
            assert_eq!(*action, ActionKind::Claim);
            let receipt = self
                .client
                .claim_evm_with_wallet(id, &self.wallet, self.options.clone(), vec![])
                .await?;
            Ok(ExecutionResult::Submitted {
                reference: receipt.transaction_hash.to_string(),
            })
        })
    }
}
#[tokio::test]
#[ignore = "requires Anvil and freshly compiled repository contracts; run scripts/e2e/test-rust-sdk-local.sh"]
async fn restored_tracker_automatically_claims_once_using_real_chain_observations() -> TestResult {
    let f = Fixture::new().await?;
    f.save_swap(false, None)?;
    let contracts = SwapContracts {
        client: None,
        server: Some(reference(&f, false)),
    };
    let tracker = SwapTracker::new(
        f.client.clone(),
        ContractManager::new(Arc::new(reader(&f).await?), ManagerOptions::default())?,
        TrackerOptions::default(),
        None,
    )?;
    tracker
        .track(
            ID,
            contracts,
            TrackingOptions {
                auto_claim: true,
                ..Default::default()
            },
        )
        .await?;
    drop(tracker);
    f.fund_target().await?;
    let key = lendaswap_sdk::Signer::from_mnemonic(MNEMONIC)?.derive_evm_key(0)?;
    let executor = Arc::new(Claimer {
        client: f.reopen("http://127.0.0.1:1")?,
        wallet: lendaswap_sdk::evm::RpcWallet::connect(&key, CHAIN, f.url.clone()).await?,
        options: f.claim_options(),
    });
    let tracker = SwapTracker::new(
        f.reopen("http://127.0.0.1:1")?,
        ContractManager::new(Arc::new(reader(&f).await?), ManagerOptions::default())?,
        TrackerOptions::default(),
        Some(executor),
    )?;
    assert!(tracker.restore().await?.is_empty());
    let result = tracker.reconcile(ID).await?;
    assert_eq!(
        result.recommended().unwrap().kind,
        ActionKind::Complete {
            outcome: Outcome::Completed
        }
    );
    let nonce = f
        .rpc("eth_getTransactionCount", json!([f.owner, "latest"]))
        .await?;
    tracker.reconcile(ID).await?;
    assert_eq!(
        f.rpc("eth_getTransactionCount", json!([f.owner, "latest"]))
            .await?,
        nonce
    );
    assert_eq!(f.balance(f.owner).await?, U256::from(AMOUNT));
    Ok(())
}

#[tokio::test]
#[ignore = "requires Anvil and freshly compiled repository contracts; run scripts/e2e/test-rust-sdk-local.sh"]
async fn native_htlc_claim_verifies_on_chain_and_pays_the_coin() -> TestResult {
    let f = Fixture::new().await?;
    let htlc = f
        .deploy("HTLCNative.sol/HTLCNative.json", (f.deployer,).abi_encode())
        .await?;
    let coordinator = f
        .deploy(
            "HTLCNativeCoordinator.sol/HTLCNativeCoordinator.json",
            (htlc,).abi_encode(),
        )
        .await?;
    f.transact_value(
        f.deployer,
        htlc,
        NativeLock::createCall {
            preimageHash: f.hash,
            claimAddress: f.owner,
            timelock: U256::from(f.timelock),
        },
        U256::from(AMOUNT),
    )
    .await?;
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/swap-responses.json"))?;
    let mut value = fixtures["lightning_to_evm"].clone();
    for (key, val) in [
        ("id", json!(ID)),
        ("evm_chain_id", json!(CHAIN)),
        ("client_evm_address", json!(f.owner)),
        ("server_evm_address", json!(f.deployer)),
        ("evm_htlc_address", json!(htlc)),
        ("evm_coordinator_address", json!(coordinator)),
        ("hash_lock", json!(f.hash)),
        ("evm_refund_locktime", json!(f.timelock)),
        ("evm_htlc_version", json!(1)),
        ("evm_htlc_kind", json!("native")),
        ("wbtc_address", json!(Address::ZERO)),
        ("evm_expected_sats", json!(AMOUNT.to_string())),
    ] {
        value[key] = val;
    }
    f.store.store_swap(StoredSwap::new(
        0,
        serde_json::from_value(value)?,
        SwapMetadata::default(),
    )?)?;
    let lock = ContractRef::Evm {
        htlc: lendaswap_sdk::evm::htlc::EvmHtlc {
            chain_id: CHAIN,
            address: htlc,
            hash_lock: f.hash,
            amount: U256::from(AMOUNT),
            token: Address::ZERO,
            sender: f.deployer,
            claimer: f.owner,
            timelock: f.timelock,
        },
        from_block: 0,
        confirmations: 1,
    };
    assert_eq!(
        reader(&f).await?.observe(&lock).await?.state,
        HtlcState::Confirmed
    );
    let before = coin_balance(&f, f.owner).await?;
    let receipt = f
        .client
        .claim_evm_with_wallet(
            ID,
            &f.wallet,
            EvmClaimOptions {
                htlc,
                coordinator,
                destination: f.owner,
                sweep_token: Address::ZERO,
                min_amount_out: U256::from(AMOUNT),
                minimum_remaining_seconds: 300,
                timeout: Duration::from_secs(20),
            },
            vec![],
        )
        .await?;
    let mined = f
        .rpc(
            "eth_getTransactionReceipt",
            json!([receipt.transaction_hash]),
        )
        .await?;
    let fee = quantity(&mined["gasUsed"])? * quantity(&mined["effectiveGasPrice"])?;
    assert_eq!(
        coin_balance(&f, f.owner).await? + fee - before,
        U256::from(AMOUNT)
    );
    assert_eq!(coin_balance(&f, coordinator).await?, U256::ZERO);
    assert_eq!(
        reader(&f).await?.observe(&lock).await?.state,
        HtlcState::SpentClaim
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires Anvil and freshly compiled repository contracts; run scripts/e2e/test-rust-sdk-local.sh"]
async fn native_htlc_refund_through_the_coordinator_returns_the_coin() -> TestResult {
    let f = Fixture::new().await?;
    let htlc = f
        .deploy("HTLCNative.sol/HTLCNative.json", (f.deployer,).abi_encode())
        .await?;
    let coordinator = f
        .deploy(
            "HTLCNativeCoordinator.sol/HTLCNativeCoordinator.json",
            (htlc,).abi_encode(),
        )
        .await?;
    f.rpc("anvil_impersonateAccount", json!([f.owner])).await?;
    let funding = f
        .transact_value(
            f.owner,
            coordinator,
            NativeFunding::executeAndCreateCall {
                calls: vec![],
                preimageHash: f.hash,
                amount: U256::from(AMOUNT),
                claimAddress: f.deployer,
                timelock: U256::from(f.timelock),
            },
            U256::from(AMOUNT),
        )
        .await?;
    f.rpc("anvil_stopImpersonatingAccount", json!([f.owner]))
        .await?;
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/swap-responses.json"))?;
    let mut value = fixtures["evm_to_arkade"].clone();
    for (key, val) in [
        ("id", json!(ID)),
        ("evm_chain_id", json!(CHAIN)),
        ("client_evm_address", json!(f.owner)),
        ("server_evm_address", json!(f.deployer)),
        ("evm_htlc_address", json!(htlc)),
        ("hash_lock", json!(f.hash)),
        ("evm_refund_locktime", json!(f.timelock)),
        ("evm_htlc_version", json!(1)),
        ("evm_htlc_kind", json!("native")),
        ("evm_expected_sats", json!(AMOUNT.to_string())),
    ] {
        value[key] = val;
    }
    value["source_token"]["token_id"] = json!(Address::ZERO);
    f.store.store_swap(StoredSwap::new(
        0,
        serde_json::from_value(value)?,
        SwapMetadata {
            evm_fund_txid: Some(funding.to_string()),
            evm_coordinator_address: Some(coordinator.to_string()),
            ..Default::default()
        },
    )?)?;
    let lock = ContractRef::Evm {
        htlc: lendaswap_sdk::evm::htlc::EvmHtlc {
            chain_id: CHAIN,
            address: htlc,
            hash_lock: f.hash,
            amount: U256::from(AMOUNT),
            token: Address::ZERO,
            sender: coordinator,
            claimer: f.deployer,
            timelock: f.timelock,
        },
        from_block: 0,
        confirmations: 1,
    };
    assert_eq!(
        reader(&f).await?.observe(&lock).await?.state,
        HtlcState::Confirmed
    );
    let options = RefundOptions {
        htlc,
        lock_token: Address::ZERO,
        coordinator: Some(coordinator),
        timeout: Duration::from_secs(20),
    };
    assert!(
        f.client
            .refund_evm_with_wallet(ID, &f.wallet, options.clone())
            .await
            .is_err()
    );
    f.rpc("evm_setNextBlockTimestamp", json!([f.timelock + 1]))
        .await?;
    f.rpc("evm_mine", json!([])).await?;
    let before = coin_balance(&f, f.owner).await?;
    let receipt = f
        .client
        .refund_evm_with_wallet(ID, &f.wallet, options)
        .await?;
    let mined = f
        .rpc(
            "eth_getTransactionReceipt",
            json!([receipt.transaction_hash]),
        )
        .await?;
    let fee = quantity(&mined["gasUsed"])? * quantity(&mined["effectiveGasPrice"])?;
    assert_eq!(
        coin_balance(&f, f.owner).await? + fee - before,
        U256::from(AMOUNT)
    );
    assert_eq!(coin_balance(&f, htlc).await?, U256::ZERO);
    assert_eq!(coin_balance(&f, coordinator).await?, U256::ZERO);
    assert_eq!(
        reader(&f).await?.observe(&lock).await?.state,
        HtlcState::SpentRefund
    );
    Ok(())
}

/// The daemon's relay against the real contracts: submits the posted EIP-2612 permit
/// and Permit2 funding from the deployer, using the swap's own parameters as the
/// daemon does from its database.
#[derive(Clone)]
struct AnvilRelay {
    url: url::Url,
    deployer: Address,
    coordinator: Address,
    token: Address,
    owner: Address,
    hash: lendaswap_sdk::evm::B256,
    timelock: u64,
}
impl wiremock::Respond for AnvilRelay {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let relay = self.clone();
        // Respond is synchronous, and wiremock runs it inside its own runtime.
        let submitted = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?
                .block_on(relay.submit(body))
                .map_err(|e| e.to_string())
        })
        .join();
        match submitted {
            Ok(Ok(hash)) => ResponseTemplate::new(200)
                .set_body_json(json!({"id": ID, "tx_hash": hash, "message": "funded"})),
            Ok(Err(error)) => ResponseTemplate::new(400).set_body_json(json!({"error": error})),
            Err(_) => ResponseTemplate::new(500),
        }
    }
}
impl AnvilRelay {
    async fn submit(self, body: serde_json::Value) -> TestResult<lendaswap_sdk::evm::B256> {
        use lendaswap_sdk::aa::abi::IHTLCCoordinator;
        use lendaswap_sdk::aa::abi::PermitTransferFrom;
        use lendaswap_sdk::aa::abi::TokenPermissions;
        use lendaswap_sdk::aa::permit2::PERMIT2_ADDRESS;
        use lendaswap_sdk::evm::permit::PermitToken;
        let http = reqwest::Client::new();
        let text = |value: &serde_json::Value| -> TestResult<String> {
            Ok(value.as_str().ok_or("expected a string")?.to_owned())
        };
        let send = |to: Address, data: Vec<u8>| {
            anvil::send(
                &http,
                &self.url,
                json!({"from": self.deployer, "to": to, "data": format!("0x{}", hex::encode(data)), "gas": "0x989680"}),
            )
        };
        if let Some(permit) = body.get("eip2612_permit") {
            send(
                self.token,
                PermitToken::permitCall {
                    owner: self.owner,
                    spender: PERMIT2_ADDRESS,
                    value: U256::from_str_radix(&text(&permit["value"])?, 10)?,
                    deadline: U256::from(permit["deadline"].as_u64().ok_or("deadline")?),
                    v: u8::try_from(permit["v"].as_u64().ok_or("v")?)?,
                    r: text(&permit["r"])?.parse()?,
                    s: text(&permit["s"])?.parse()?,
                }
                .abi_encode(),
            )
            .await?;
        }
        let signature: alloy::primitives::Bytes = text(&body["permit2_signature"])?.parse()?;
        let receipt = send(
            self.coordinator,
            IHTLCCoordinator::executeAndCreateWithPermit2Call {
                calls: vec![],
                preimageHash: self.hash,
                token: self.token,
                claimAddress: self.deployer,
                timelock: U256::from(self.timelock),
                depositor: self.owner,
                permit: PermitTransferFrom {
                    permitted: TokenPermissions {
                        token: self.token,
                        amount: U256::from(AMOUNT),
                    },
                    nonce: U256::from_str_radix(&text(&body["permit2_nonce"])?, 10)?,
                    deadline: U256::from(body["permit2_deadline"].as_u64().ok_or("deadline")?),
                },
                signature,
            }
            .abi_encode(),
        )
        .await?;
        Ok(text(&receipt["transactionHash"])?.parse()?)
    }
}

/// A gasless swap from an EIP-2612 token with no Permit2 allowance, locked as is, so
/// the relay needs both signatures and no DEX. The depositor holds `deposit` of it.
/// Returns a client whose API relays the funding through Anvil once, the API, which
/// must stay alive, and the token.
async fn relay_swap(
    f: &Fixture,
    deposit: u64,
) -> TestResult<(lendaswap_sdk::Client, MockServer, Address)> {
    let usdc = f
        .deploy("HTLCCoordinatorSwapAndLock.t.sol/MockUSDC.json", vec![])
        .await?;
    f.save_swap(true, None)?;
    let mut stored = f.store.get_swap(ID)?.ok_or("stored swap missing")?;
    let mut response = serde_json::to_value(&stored.response)?;
    response["gasless"] = json!(true);
    response["source_token"]["token_id"] = json!(usdc);
    stored.response = serde_json::from_value(response.clone())?;
    f.store.store_swap(stored)?;
    f.transact(
        f.deployer,
        usdc,
        Token::transferCall {
            to: f.owner,
            amount: U256::from(deposit),
        },
    )
    .await?;
    let separator = f
        .word(
            usdc,
            lendaswap_sdk::evm::permit::PermitToken::DOMAIN_SEPARATORCall {}.abi_encode(),
        )
        .await?;
    let calls: Vec<Call> = vec![];
    let payload = json!({
        "coordinator_address": f.coordinator,
        "permit2_address": lendaswap_sdk::aa::permit2::PERMIT2_ADDRESS,
        "source_token_address": usdc,
        "source_amount": AMOUNT.to_string(),
        "lock_token_address": usdc,
        "preimage_hash": f.hash,
        "claim_address": f.deployer,
        "timelock": f.timelock,
        "calls": [],
        "calls_hash": keccak256(calls.abi_encode()),
        "eip2612": {"supported": true, "already_approved": false, "nonce": 0, "domain_separator": lendaswap_sdk::evm::B256::from(separator.to_be_bytes::<32>())},
        "relay_fee": "0",
        "depositor_delegation": null
    });
    let api = MockServer::start().await;
    Mock::given(path(format!("/swap/{ID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(&response))
        .mount(&api)
        .await;
    Mock::given(path(format!("/swap/{ID}/swap-and-lock-calldata-permit2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(payload))
        .mount(&api)
        .await;
    Mock::given(path(format!("/swap/{ID}/fund-gasless")))
        .respond_with(AnvilRelay {
            url: f.url.clone(),
            deployer: f.deployer,
            coordinator: f.coordinator,
            token: usdc,
            owner: f.owner,
            hash: f.hash,
            timelock: f.timelock,
        })
        .expect(1)
        .mount(&api)
        .await;
    Ok((f.reopen(&api.uri())?, api, usdc))
}

fn relay_options(f: &Fixture) -> RelayFundingOptions {
    let mut options = RelayFundingOptions::new(U256::ZERO);
    options.node_rpc_url = Some(f.url.clone());
    options.timeout = Duration::from_secs(30);
    options
}

#[tokio::test]
#[ignore = "requires Anvil and freshly compiled repository contracts; run scripts/e2e/test-rust-sdk-local.sh"]
async fn relay_funding_signatures_are_accepted_by_real_permit2_and_coordinator() -> TestResult {
    let f = Fixture::new().await?;
    let (client, _api, usdc) = relay_swap(&f, AMOUNT).await?;
    let usdc_balance =
        |account: Address| f.word(usdc, Token::balanceOfCall { account }.abi_encode());
    let funded = client
        .fund_swap_gasless_relay(ID, relay_options(&f))
        .await?;
    assert!(funded.eip2612_permit);
    assert_eq!(funded.htlc.amount, U256::from(AMOUNT));
    assert_eq!(usdc_balance(f.owner).await?, U256::ZERO);
    assert_eq!(usdc_balance(f.htlc).await?, U256::from(AMOUNT));
    assert_eq!(
        client
            .get_stored_swap(ID)?
            .ok_or("stored swap missing")?
            .metadata
            .evm_fund_txid,
        Some(funded.tx_hash.to_string())
    );
    let refundable = client
        .get_funded_evm_htlc(
            ID,
            &f.wallet,
            &RefundOptions {
                htlc: f.htlc,
                lock_token: usdc,
                coordinator: Some(f.coordinator),
                timeout: Duration::from_secs(20),
            },
        )
        .await?;
    assert_eq!(refundable, funded.htlc);
    Ok(())
}

#[tokio::test]
#[ignore = "requires Anvil and freshly compiled repository contracts; run scripts/e2e/test-rust-sdk-local.sh"]
async fn a_relay_funded_depositor_sweeps_and_refunds_paying_its_own_gas() -> TestResult {
    let f = Fixture::new().await?;
    let excess = 500;
    let (client, _api, usdc) = relay_swap(&f, AMOUNT + excess).await?;
    let usdc_balance =
        |account: Address| f.word(usdc, Token::balanceOfCall { account }.abi_encode());
    let nonce = || f.rpc("eth_getTransactionCount", json!([f.owner, "latest"]));
    let (destination, timeout) = (Address::repeat_byte(0x77), Duration::from_secs(30));
    let sweep = || client.sweep_evm_deposit(ID, None, destination, Some(&f.url), timeout);
    client
        .fund_swap_gasless_relay(ID, relay_options(&f))
        .await?;
    assert_eq!(
        client
            .depositor_balance(ID, None, Some(&f.url))
            .await?
            .token_units,
        U256::from(excess)
    );
    // The relay funding is proven, so no signature can pull the excess any more.
    sweep().await?;
    assert_eq!(usdc_balance(destination).await?, U256::from(excess));
    assert_eq!(usdc_balance(f.owner).await?, U256::ZERO);

    // A later deposit stays put while the depositor has no gas.
    f.transact(
        f.deployer,
        usdc,
        Token::transferCall {
            to: f.owner,
            amount: U256::from(7),
        },
    )
    .await?;
    let gas = f.rpc("eth_getBalance", json!([f.owner, "latest"])).await?;
    for poor in ["0x0", "0x1"] {
        f.rpc("anvil_setBalance", json!([f.owner, poor])).await?;
        let before = nonce().await?;
        let refused = sweep().await;
        assert!(
            matches!(&refused, Err(lendaswap_sdk::Error::InsufficientGas(_))),
            "{refused:?}"
        );
        assert_eq!(nonce().await?, before);
    }
    f.rpc("anvil_setBalance", json!([f.owner, gas])).await?;
    sweep().await?;
    assert_eq!(usdc_balance(destination).await?, U256::from(excess + 7));

    // The HTLC refund waits for its locktime, then pays the lock token back once.
    let early = client.refund_evm_htlc(ID, Some(&f.url), timeout).await;
    assert!(
        matches!(&early, Err(lendaswap_sdk::Error::InvalidSwap(message)) if message.contains("timelock")),
        "{early:?}"
    );
    f.rpc("evm_setNextBlockTimestamp", json!([f.timelock + 1]))
        .await?;
    f.rpc("evm_mine", json!([])).await?;
    let refund = client.refund_evm_htlc(ID, Some(&f.url), timeout).await?;
    assert_eq!(refund.lock_token, usdc);
    assert_eq!(usdc_balance(f.owner).await?, U256::from(AMOUNT));
    let before = nonce().await?;
    assert_eq!(
        client
            .refund_evm_htlc(ID, Some(&f.url), timeout)
            .await?
            .receipt
            .transaction_hash,
        refund.receipt.transaction_hash
    );
    assert_eq!(nonce().await?, before);
    sweep().await?;
    assert_eq!(
        usdc_balance(destination).await?,
        U256::from(AMOUNT + excess + 7)
    );
    Ok(())
}
