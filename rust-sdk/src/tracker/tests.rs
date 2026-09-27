use super::*;
use crate::InMemorySwapStorage;
use crate::StoredSwap;
use crate::SwapStorage;
use crate::actions::tests::observation;
use crate::actions::tests::reference;
use crate::observation::ContractReader;
use crate::observation::ContractRef;
use crate::observation::HtlcState;
use crate::observation::ManagerOptions;
use crate::observation::Observation;
use crate::observation::ObservationFuture;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
struct Reader(Mutex<(HtlcState, HtlcState, u64)>, AtomicBool);
impl ContractReader for Reader {
    fn can_observe(&self, _: &ContractRef) -> bool {
        true
    }
    fn observe<'a>(&'a self, reference: &'a ContractRef) -> ObservationFuture<'a, Observation> {
        Box::pin(async move {
            if self.1.swap(false, Ordering::SeqCst) {
                panic!("reader bug");
            }
            let (client, server, height) = *self.0.lock().unwrap();
            Ok(observation(
                if reference.timelock() == crate::observation::Timelock::Height(200) {
                    client
                } else {
                    server
                },
                height,
            ))
        })
    }
}
struct Executor {
    calls: AtomicUsize,
    mode: u8,
    store: Arc<dyn SwapStorage>,
}
impl ActionExecutor for Executor {
    fn execute<'a>(&'a self, id: &'a str, _: &'a ActionKind) -> ExecutionFuture<'a> {
        Box::pin(async move {
            let record: TrackingRecord = serde_json::from_value(
                self.store.get_swap(id)?.unwrap().metadata.tracking.unwrap(),
            )
            .unwrap();
            assert!(matches!(
                record.attempt.unwrap().state,
                AttemptState::Running
            ));
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.mode {
                0 => Ok(ExecutionResult::Submitted {
                    reference: "claim-tx".into(),
                }),
                1 => Err(Error::Transport("response lost".into())),
                2 => Ok(ExecutionResult::Retryable),
                4 => panic!("app bug"),
                5 => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Ok(ExecutionResult::Submitted {
                        reference: "claim-tx".into(),
                    })
                }
                _ => std::future::pending().await,
            }
        })
    }
}
fn setup(mode: u8, options: TrackerOptions) -> (SwapTracker, Arc<Reader>, Arc<Executor>, String) {
    let store = InMemorySwapStorage::shared();
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let record = StoredSwap::new(
        0,
        serde_json::from_value(fixtures["btc_to_arkade"].clone()).unwrap(),
        SwapMetadata::default(),
    )
    .unwrap();
    let id = record.response.id().to_string();
    store.store_swap(record).unwrap();
    let client = Client::builder().mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").storage(store.clone()).build().unwrap();
    let reader = Arc::new(Reader(
        Mutex::new((HtlcState::Absent, HtlcState::Absent, 50)),
        AtomicBool::new(false),
    ));
    let manager = ContractManager::new(reader.clone(), ManagerOptions::default()).unwrap();
    let executor = Arc::new(Executor {
        calls: AtomicUsize::new(0),
        mode,
        store,
    });
    (
        SwapTracker::new(client, manager, options, Some(executor.clone())).unwrap(),
        reader,
        executor,
        id,
    )
}
async fn track(tracker: &SwapTracker, id: &str) {
    tracker
        .track(
            id,
            SwapContracts {
                client: Some(reference(200)),
                server: Some(reference(100)),
            },
            TrackingOptions {
                auto_claim: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
}
#[tokio::test]
async fn lifecycle_restores_without_duplicate_claim_and_follows_reorgs() {
    let (tracker, reader, executor, id) = setup(0, TrackerOptions::default());
    track(&tracker, &id).await;
    assert_eq!(
        tracker
            .reconcile(&id)
            .await
            .unwrap()
            .recommended()
            .unwrap()
            .kind,
        ActionKind::Fund
    );
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Absent, 50);
    assert_eq!(
        tracker
            .reconcile(&id)
            .await
            .unwrap()
            .recommended()
            .unwrap()
            .kind,
        ActionKind::Wait {
            on: WaitingOn::ServerFunding
        }
    );
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Confirmed, 50);
    let (a, b) = tokio::join!(tracker.reconcile(&id), tracker.reconcile(&id));
    a.unwrap();
    b.unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    let restarted = SwapTracker::new(
        tracker.0.client.clone(),
        ContractManager::new(reader.clone(), ManagerOptions::default()).unwrap(),
        TrackerOptions::default(),
        Some(executor.clone()),
    )
    .unwrap();
    assert!(restarted.restore().await.unwrap().is_empty());
    assert_eq!(
        restarted
            .reconcile(&id)
            .await
            .unwrap()
            .recommended()
            .unwrap()
            .kind,
        ActionKind::Wait {
            on: WaitingOn::ClaimConfirmation
        }
    );
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::SpentClaim, 51);
    assert_eq!(
        restarted
            .reconcile(&id)
            .await
            .unwrap()
            .recommended()
            .unwrap()
            .kind,
        ActionKind::Complete {
            outcome: Outcome::Completed
        }
    );
    assert!(restarted.record(&id).await.unwrap().completed_ms.is_some());
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Confirmed, 50);
    assert_eq!(
        restarted
            .reconcile(&id)
            .await
            .unwrap()
            .recommended()
            .unwrap()
            .kind,
        ActionKind::Wait {
            on: WaitingOn::ClaimConfirmation
        }
    );
    assert!(restarted.record(&id).await.unwrap().completed_ms.is_none());
    restarted.untrack(&id).await.unwrap();
    assert!(
        !restarted
            .0
            .client
            .get_stored_swap(&id)
            .unwrap()
            .unwrap()
            .metadata
            .tracking
            .unwrap()["enabled"]
            .as_bool()
            .unwrap()
    );
    restarted.resume(&id).await.unwrap();
    restarted.reconcile(&id).await.unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn refund_path_never_automates_even_with_executor_enabled() {
    let (tracker, reader, executor, id) = setup(0, TrackerOptions::default());
    track(&tracker, &id).await;
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::SpentRefund, 200);
    assert_eq!(
        tracker
            .reconcile(&id)
            .await
            .unwrap()
            .recommended()
            .unwrap()
            .kind,
        ActionKind::RefundUnilateral
    );
    *reader.0.lock().unwrap() = (HtlcState::SpentRefund, HtlcState::SpentRefund, 201);
    assert_eq!(
        tracker
            .reconcile(&id)
            .await
            .unwrap()
            .recommended()
            .unwrap()
            .kind,
        ActionKind::Complete {
            outcome: Outcome::Refunded
        }
    );
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn terminal_swaps_retire_and_free_capacity() {
    let options = || TrackerOptions {
        max_swaps: 1,
        retire_after: Duration::ZERO,
        ..Default::default()
    };
    let (tracker, reader, _, id) = setup(0, options());
    track(&tracker, &id).await;
    *reader.0.lock().unwrap() = (HtlcState::SpentRefund, HtlcState::SpentRefund, 201);
    assert_eq!(
        tracker
            .reconcile(&id)
            .await
            .unwrap()
            .recommended()
            .unwrap()
            .kind,
        ActionKind::Complete {
            outcome: Outcome::Refunded
        }
    );
    assert!(tracker.record(&id).await.is_err());
    assert!(tracker.actions(&id).is_err());
    let journal: TrackingRecord =
        serde_json::from_value(stored_metadata(&tracker, &id).tracking.unwrap()).unwrap();
    assert!(!journal.enabled);
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let mut response = fixtures["btc_to_arkade"].clone();
    response["id"] = "00000000-0000-0000-0000-000000000002".into();
    let stored = StoredSwap::new(
        1,
        serde_json::from_value(response).unwrap(),
        SwapMetadata::default(),
    )
    .unwrap();
    let second = stored.response.id().to_string();
    tracker.0.client.storage.store_swap(stored).unwrap();
    track(&tracker, &second).await;
    let restarted = SwapTracker::new(
        tracker.0.client.clone(),
        ContractManager::new(reader.clone(), ManagerOptions::default()).unwrap(),
        options(),
        None,
    )
    .unwrap();
    assert!(restarted.restore().await.unwrap().is_empty());
    assert!(restarted.record(&id).await.is_err());
    assert!(restarted.record(&second).await.unwrap().enabled);
}
#[tokio::test]
async fn an_expired_swap_with_an_unminted_cctp_burn_is_not_retired() {
    let (tracker, reader, _, id) = setup(
        0,
        TrackerOptions {
            retire_after: Duration::ZERO,
            ..Default::default()
        },
    );
    track(&tracker, &id).await;
    tracker
        .update_options(
            &id,
            TrackingOptions {
                bridge: BridgeState::CctpBurn {
                    source_transaction: "burn".into(),
                    attestation_available: true,
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
    *reader.0.lock().unwrap() = (HtlcState::Absent, HtlcState::Absent, 206);
    let actions = tracker.reconcile(&id).await.unwrap();
    assert_eq!(
        actions.recommended().unwrap().kind,
        ActionKind::Complete {
            outcome: Outcome::Expired
        }
    );
    assert!(tracker.record(&id).await.unwrap().enabled);
    // A completed swap was funded, so its burn was minted and it retires.
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::SpentClaim, 206);
    tracker.reconcile(&id).await.unwrap();
    assert!(tracker.record(&id).await.is_err());
}
#[tokio::test]
async fn resumed_swap_gets_a_fresh_retention_window() {
    let (tracker, reader, _, id) = setup(
        0,
        TrackerOptions {
            retire_after: Duration::from_millis(300),
            ..Default::default()
        },
    );
    track(&tracker, &id).await;
    *reader.0.lock().unwrap() = (HtlcState::SpentRefund, HtlcState::SpentRefund, 201);
    tracker.reconcile(&id).await.unwrap();
    assert!(tracker.record(&id).await.unwrap().enabled);
    tokio::time::sleep(Duration::from_millis(350)).await;
    tracker.reconcile(&id).await.unwrap();
    assert!(tracker.record(&id).await.is_err());
    tracker.resume(&id).await.unwrap();
    tracker.reconcile(&id).await.unwrap();
    assert!(tracker.record(&id).await.unwrap().completed_ms.is_some());
}
#[tokio::test]
async fn ambiguous_errors_require_explicit_reconciliation_and_retries_are_bounded() {
    for mode in [1, 2] {
        let (tracker, reader, executor, id) = setup(
            mode,
            TrackerOptions {
                retry_delay: Duration::from_millis(1),
                max_attempts: 2,
                ..Default::default()
            },
        );
        track(&tracker, &id).await;
        *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Confirmed, 50);
        for _ in 0..5 {
            tracker.reconcile(&id).await.unwrap();
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(
            executor.calls.load(Ordering::SeqCst),
            if mode == 1 { 1 } else { 2 }
        );
        if mode == 1 {
            assert_eq!(
                tracker
                    .actions(&id)
                    .unwrap()
                    .unwrap()
                    .recommended()
                    .unwrap()
                    .kind,
                ActionKind::Wait {
                    on: WaitingOn::SubmissionRecovery
                }
            );
            tracker
                .resolve_attempt(&id, AttemptResolution::ProvenNotSubmitted)
                .await
                .unwrap();
            tracker.reconcile(&id).await.unwrap();
            assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
        }
    }
}
#[tokio::test]
async fn cancellation_preserves_reservation_and_hints_are_coalesced() {
    let (tracker, reader, executor, id) = setup(3, TrackerOptions::default());
    track(&tracker, &id).await;
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Confirmed, 50);
    for _ in 0..1000 {
        tracker.hint(&id).unwrap();
        tracker.hint("untracked").unwrap();
    }
    assert_eq!(tracker.0.pending.lock().unwrap().len(), 1);
    let worker = tracker.start().unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while executor.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), worker.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        tracker.record(&id).await.unwrap().attempt.unwrap().state,
        AttemptState::Running
    ));
    tracker.reconcile(&id).await.unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn changing_contract_expectations_is_rejected_without_reusing_cached_observation() {
    let (tracker, _, _, id) = setup(0, TrackerOptions::default());
    let mut changed = reference(200);
    if let ContractRef::Bitcoin { expected_sats, .. } = &mut changed {
        *expected_sats = 2000;
    }
    tracker.0.manager.register_unread(changed).unwrap();
    assert!(
        tracker
            .track(
                &id,
                SwapContracts {
                    client: Some(reference(200)),
                    server: None
                },
                TrackingOptions::default()
            )
            .await
            .is_err()
    );
    assert!(
        tracker
            .0
            .client
            .get_stored_swap(&id)
            .unwrap()
            .unwrap()
            .metadata
            .tracking
            .is_none()
    );
}

#[tokio::test]
async fn cctp_recovery_waits_for_observed_funding_before_claiming() {
    let (tracker, reader, executor, id) = setup(0, TrackerOptions::default());
    track(&tracker, &id).await;
    tracker
        .update_options(
            &id,
            TrackingOptions {
                auto_claim: true,
                auto_cctp: true,
                bridge: BridgeState::CctpBurn {
                    source_transaction: "burn".into(),
                    attestation_available: true,
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        tracker
            .reconcile(&id)
            .await
            .unwrap()
            .recommended()
            .unwrap()
            .kind,
        ActionKind::Wait {
            on: WaitingOn::FundingConfirmation
        }
    );
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    tracker.reconcile(&id).await.unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Confirmed, 50);
    assert_eq!(
        tracker
            .reconcile(&id)
            .await
            .unwrap()
            .recommended()
            .unwrap()
            .kind,
        ActionKind::Wait {
            on: WaitingOn::ClaimConfirmation
        }
    );
    assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn sqlite_reopen_retains_timed_out_attempt_and_opt_in_options() {
    let (tracker, reader, executor, id) = setup(
        3,
        TrackerOptions {
            execution_timeout: Duration::from_millis(5),
            ..Default::default()
        },
    );
    track(&tracker, &id).await;
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Confirmed, 50);
    tracker.reconcile(&id).await.unwrap();
    assert!(matches!(
        tracker.record(&id).await.unwrap().attempt.unwrap().state,
        AttemptState::Uncertain
    ));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("swaps.db");
    {
        let store = crate::SqliteSwapStorage::open(&path, "wallet").unwrap();
        store
            .store_swap(tracker.0.client.get_stored_swap(&id).unwrap().unwrap())
            .unwrap();
    }
    let store = Arc::new(crate::SqliteSwapStorage::open(&path, "wallet").unwrap());
    let client = Client::builder().mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").storage(store).build().unwrap();
    let restarted = SwapTracker::new(
        client,
        ContractManager::new(reader, ManagerOptions::default()).unwrap(),
        TrackerOptions::default(),
        Some(executor.clone()),
    )
    .unwrap();
    assert!(restarted.restore().await.unwrap().is_empty());
    assert!(restarted.record(&id).await.unwrap().options.auto_claim);
    restarted.reconcile(&id).await.unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn restore_reports_unreadable_sqlite_rows_and_installs_the_rest() {
    let (tracker, reader, executor, id) = setup(0, TrackerOptions::default());
    track(&tracker, &id).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("swaps.db");
    let store = Arc::new(crate::SqliteSwapStorage::open(&path, "wallet").unwrap());
    store
        .store_swap(tracker.0.client.get_stored_swap(&id).unwrap().unwrap())
        .unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO sdk_swap_records VALUES ('wallet', 'corrupt', '{not json')",
            [],
        )
        .unwrap();
    let client = Client::builder().mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").storage(store).build().unwrap();
    let restarted = SwapTracker::new(
        client,
        ContractManager::new(reader, ManagerOptions::default()).unwrap(),
        TrackerOptions::default(),
        Some(executor),
    )
    .unwrap();
    let failures = restarted.restore().await.unwrap();
    assert_eq!(
        failures
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        ["corrupt"]
    );
    assert!(restarted.record(&id).await.unwrap().enabled);
}

fn unsent_operation() -> crate::storage::BridgeOperation {
    crate::storage::BridgeOperation {
        intent: serde_json::json!({}),
        transaction_hash: None,
        user_operation_hash: None,
        complete: false,
    }
}

fn stored_metadata(tracker: &SwapTracker, id: &str) -> SwapMetadata {
    tracker
        .0
        .client
        .get_stored_swap(id)
        .unwrap()
        .unwrap()
        .metadata
}

#[tokio::test]
async fn proven_not_submitted_releases_only_the_attempted_claim_rail() {
    let (tracker, reader, _, id) = setup(1, TrackerOptions::default());
    track(&tracker, &id).await;
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Confirmed, 50);
    tracker.reconcile(&id).await.unwrap();
    assert!(matches!(
        tracker.record(&id).await.unwrap().attempt.unwrap(),
        Attempt {
            action: ActionKind::Claim,
            state: AttemptState::Uncertain,
            ..
        }
    ));
    tracker
        .0
        .client
        .update_swap_metadata(
            &id,
            SwapMetadata {
                evm_claim_started: Some(true),
                evm_fund_started: Some(true),
                evm_recovery_started: Some(true),
                bridge_operations: Some([("cctp_fund".into(), unsent_operation())].into()),
                ..Default::default()
            },
        )
        .unwrap();
    tracker
        .resolve_attempt(&id, AttemptResolution::ProvenNotSubmitted)
        .await
        .unwrap();
    let metadata = stored_metadata(&tracker, &id);
    assert_eq!(metadata.evm_claim_started, Some(false));
    assert_eq!(metadata.evm_fund_started, Some(true));
    assert_eq!(metadata.evm_recovery_started, Some(true));
    assert!(
        metadata
            .bridge_operations
            .unwrap()
            .contains_key("cctp_fund")
    );
    assert!(tracker.record(&id).await.unwrap().attempt.is_none());
    assert!(
        tracker
            .resolve_attempt(&id, AttemptResolution::ProvenNotSubmitted)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn proven_not_submitted_recover_cctp_drops_only_hashless_cctp_operations() {
    for sent in [false, true] {
        let (tracker, _, _, id) = setup(1, TrackerOptions::default());
        track(&tracker, &id).await;
        tracker
            .update_options(
                &id,
                TrackingOptions {
                    auto_claim: true,
                    auto_cctp: true,
                    bridge: BridgeState::CctpBurn {
                        source_transaction: "burn".into(),
                        attestation_available: true,
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        tracker.reconcile(&id).await.unwrap();
        assert!(matches!(
            tracker.record(&id).await.unwrap().attempt.unwrap(),
            Attempt {
                action: ActionKind::RecoverCctp { .. },
                state: AttemptState::Uncertain,
                ..
            }
        ));
        let burn = crate::storage::BridgeOperation {
            transaction_hash: Some("0xburn".into()),
            ..unsent_operation()
        };
        let (fund, mint) = if sent {
            (
                crate::storage::BridgeOperation {
                    user_operation_hash: Some("0xfund".into()),
                    ..unsent_operation()
                },
                crate::storage::BridgeOperation {
                    transaction_hash: Some("0xmint".into()),
                    ..unsent_operation()
                },
            )
        } else {
            (unsent_operation(), unsent_operation())
        };
        tracker
            .0
            .client
            .update_swap_metadata(
                &id,
                SwapMetadata {
                    evm_claim_started: Some(true),
                    evm_fund_started: Some(true),
                    evm_recovery_started: Some(true),
                    bridge_operations: Some(
                        [
                            ("cctp_fund".into(), fund.clone()),
                            ("cctp_mint".into(), mint.clone()),
                            ("cctp_burn".into(), burn.clone()),
                        ]
                        .into(),
                    ),
                    ..Default::default()
                },
            )
            .unwrap();
        tracker
            .resolve_attempt(&id, AttemptResolution::ProvenNotSubmitted)
            .await
            .unwrap();
        let metadata = stored_metadata(&tracker, &id);
        let operations = metadata.bridge_operations.unwrap();
        assert_eq!(operations.get("cctp_fund"), sent.then_some(&fund));
        assert_eq!(operations.get("cctp_mint"), sent.then_some(&mint));
        assert_eq!(operations.get("cctp_burn"), Some(&burn));
        assert_eq!(metadata.evm_fund_started, Some(sent));
        assert_eq!(metadata.evm_claim_started, Some(true));
        assert_eq!(metadata.evm_recovery_started, Some(true));
    }
}

#[tokio::test]
async fn panicking_executor_records_uncertain_for_that_swap() {
    let (tracker, reader, executor, id) = setup(4, TrackerOptions::default());
    track(&tracker, &id).await;
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Confirmed, 50);
    let mut events = tracker.events();
    tracker.reconcile(&id).await.unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        tracker.record(&id).await.unwrap().attempt.unwrap().state,
        AttemptState::Uncertain
    ));
    let mut reported = false;
    while let Ok(event) = events.try_recv() {
        reported |=
            event.swap_id == id && event.error.is_some_and(|error| error.contains("panicked"));
    }
    assert!(reported);
}

#[tokio::test]
async fn panicking_reconcile_does_not_abort_siblings() {
    let (tracker, reader, _, id) = setup(
        5,
        TrackerOptions {
            concurrency: 2,
            ..Default::default()
        },
    );
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let mut response = fixtures["btc_to_arkade"].clone();
    response["id"] = "00000000-0000-0000-0000-000000000002".into();
    let stored = StoredSwap::new(
        1,
        serde_json::from_value(response).unwrap(),
        SwapMetadata::default(),
    )
    .unwrap();
    let second = stored.response.id().to_string();
    tracker.0.client.storage.store_swap(stored).unwrap();
    track(&tracker, &id).await;
    track(&tracker, &second).await;
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Confirmed, 50);
    reader.1.store(true, Ordering::SeqCst);
    assert!(tracker.refresh(false).await.is_err());
    let mut states = Vec::new();
    for swap in [&id, &second] {
        states.push(tracker.record(swap).await.unwrap().attempt.map(|a| a.state));
    }
    assert!(
        states.iter().any(Option::is_none),
        "one reconcile hit the reader panic: {states:?}"
    );
    assert!(
        states
            .iter()
            .any(|state| matches!(state, Some(AttemptState::Submitted { .. }))),
        "the sibling finished its execution: {states:?}"
    );
}
