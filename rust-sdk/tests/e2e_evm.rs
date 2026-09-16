//! Local-chain E2E tests against freshly built repository contracts and a fresh
//! Anvil per test. No fork RPC, private credentials, Docker or production funds.
//! API snapshots and the quote response are fixtures; no chain RPC is mocked.
//! Run `scripts/e2e/test-rust-sdk-local.sh` from the repository root.
#![cfg(feature = "gasless")]
#[path = "support/anvil.rs"]
mod anvil;
use alloy::primitives::keccak256;
use alloy::sol_types::SolValue;
use anvil::*;
use lendaswap_sdk::aa::abi::Call;
use lendaswap_sdk::actions::ActionKind;
use lendaswap_sdk::actions::Outcome;
use lendaswap_sdk::evm::FundingOptions;
use lendaswap_sdk::evm::RefundOptions;
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
