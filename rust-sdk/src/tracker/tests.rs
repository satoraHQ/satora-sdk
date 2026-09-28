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
                6 => Ok(ExecutionResult::NotSent(Error::Transport(
                    "connection refused".into(),
                ))),
                _ => std::future::pending().await,
            }
        })
    }
}
fn setup(mode: u8, options: TrackerOptions) -> (SwapTracker, Arc<Reader>, Arc<Executor>, String) {
    setup_fixture("btc_to_arkade", mode, options)
}
fn setup_fixture(
    fixture: &str,
    mode: u8,
    options: TrackerOptions,
) -> (SwapTracker, Arc<Reader>, Arc<Executor>, String) {
    let store = InMemorySwapStorage::shared();
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let record = StoredSwap::new(
        0,
        serde_json::from_value(fixtures[fixture].clone()).unwrap(),
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
async fn track_lightning(tracker: &SwapTracker, id: &str) {
    tracker
        .track(
            id,
            SwapContracts {
                client: None,
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
fn set_stored_status(tracker: &SwapTracker, id: &str, status: crate::types::SwapStatus) {
    let mut stored = tracker.0.client.get_stored_swap(id).unwrap().unwrap();
    let crate::types::GetSwapResponse::LightningToArkade(response) = &mut stored.response else {
        panic!("expected a Lightning to Arkade swap");
    };
    response.status = status;
    tracker.0.client.storage.store_swap(stored).unwrap();
}
fn recommended(actions: &SwapActions) -> ActionKind {
    actions.recommended().unwrap().kind.clone()
}
const REFUNDED: ActionKind = ActionKind::Complete {
    outcome: Outcome::Refunded,
};
const CLIENT_PAYMENT: ActionKind = ActionKind::Wait {
    on: WaitingOn::ClientPayment,
};
#[tokio::test]
async fn a_lightning_source_past_its_locktime_retires_without_being_at_risk() {
    let (tracker, reader, executor, id) = setup_fixture(
        "lightning_to_arkade",
        0,
        TrackerOptions {
            retire_after: Duration::ZERO,
            ..Default::default()
        },
    );
    track_lightning(&tracker, &id).await;
    *reader.0.lock().unwrap() = (HtlcState::Absent, HtlcState::Confirmed, 99);
    let actions = tracker.reconcile(&id).await.unwrap();
    assert_eq!(
        actions.recommended().unwrap().kind,
        ActionKind::Wait {
            on: WaitingOn::LightningResolution
        }
    );
    assert!(actions.at_risk);
    assert!(tracker.record(&id).await.unwrap().completed_ms.is_none());
    *reader.0.lock().unwrap() = (HtlcState::Absent, HtlcState::Confirmed, 100);
    let actions = tracker.reconcile(&id).await.unwrap();
    assert_eq!(
        actions.recommended().unwrap().kind,
        ActionKind::Complete {
            outcome: Outcome::Refunded
        }
    );
    assert!(!actions.at_risk);
    assert!(tracker.record(&id).await.is_err());
    let journal: TrackingRecord =
        serde_json::from_value(stored_metadata(&tracker, &id).tracking.unwrap()).unwrap();
    assert!(!journal.enabled);
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn a_lightning_source_status_hint_ends_and_retires_tracking() {
    let (tracker, _, executor, id) = setup_fixture(
        "lightning_to_arkade",
        0,
        TrackerOptions {
            retire_after: Duration::ZERO,
            ..Default::default()
        },
    );
    tracker
        .hint_status("untracked", crate::types::SwapStatus::ServerWontFund)
        .unwrap();
    assert!(tracker.0.pending.lock().unwrap().is_empty());
    track_lightning(&tracker, &id).await;
    assert_eq!(
        recommended(&tracker.reconcile(&id).await.unwrap()),
        CLIENT_PAYMENT
    );
    tracker.0.pending.lock().unwrap().clear();
    tracker
        .hint_status(&id, crate::types::SwapStatus::ServerWontFund)
        .unwrap();
    assert!(tracker.0.pending.lock().unwrap().contains(&id));
    let actions = tracker.reconcile(&id).await.unwrap();
    assert_eq!(recommended(&actions), REFUNDED);
    assert!(!actions.at_risk);
    assert!(tracker.record(&id).await.is_err());
    let journal: TrackingRecord =
        serde_json::from_value(stored_metadata(&tracker, &id).tracking.unwrap()).unwrap();
    assert!(!journal.enabled);
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    // A retired swap is untracked, so a later hint is ignored.
    tracker
        .hint_status(&id, crate::types::SwapStatus::ServerWontFund)
        .unwrap();
    assert!(tracker.0.pending.lock().unwrap().is_empty());
}
#[cfg(feature = "websocket")]
#[tokio::test]
async fn a_pushed_lightning_source_status_ends_tracking() {
    use crate::subscription::StatusEvent;
    use crate::subscription::StatusSource;
    let (tracker, _, _, id) = setup_fixture("lightning_to_arkade", 0, TrackerOptions::default());
    track_lightning(&tracker, &id).await;
    assert_eq!(
        recommended(&tracker.reconcile(&id).await.unwrap()),
        CLIENT_PAYMENT
    );
    tracker
        .status_hint(&StatusEvent::Update {
            swap_id: id.clone(),
            status: crate::types::SwapStatus::Expired,
            source: StatusSource::WebSocket,
        })
        .unwrap();
    assert_eq!(
        recommended(&tracker.reconcile(&id).await.unwrap()),
        ActionKind::Complete {
            outcome: Outcome::Expired
        }
    );
}
async fn restart(
    tracker: &SwapTracker,
    reader: &Arc<Reader>,
    executor: &Arc<Executor>,
) -> SwapTracker {
    let restarted = SwapTracker::new(
        tracker.0.client.clone(),
        ContractManager::new(reader.clone(), ManagerOptions::default()).unwrap(),
        TrackerOptions::default(),
        Some(executor.clone()),
    )
    .unwrap();
    assert!(restarted.restore().await.unwrap().is_empty());
    restarted
}
#[tokio::test]
async fn a_stored_failure_status_ends_a_lightning_source_swap_until_a_newer_hint() {
    let (tracker, reader, executor, id) =
        setup_fixture("lightning_to_arkade", 0, TrackerOptions::default());
    set_stored_status(&tracker, &id, crate::types::SwapStatus::ClientFundedTooLate);
    track_lightning(&tracker, &id).await;
    assert_eq!(
        recommended(&tracker.reconcile(&id).await.unwrap()),
        REFUNDED
    );
    let restarted = restart(&tracker, &reader, &executor).await;
    assert_eq!(
        recommended(&restarted.reconcile(&id).await.unwrap()),
        REFUNDED
    );
    assert!(restarted.record(&id).await.unwrap().completed_ms.is_some());
    restarted
        .hint_status(&id, crate::types::SwapStatus::Pending)
        .unwrap();
    assert_eq!(
        recommended(&restarted.reconcile(&id).await.unwrap()),
        CLIENT_PAYMENT
    );
    assert!(restarted.record(&id).await.unwrap().completed_ms.is_none());
}
#[tokio::test]
async fn hinted_statuses_survive_a_restart_without_restarting_retirement() {
    let (tracker, reader, executor, id) =
        setup_fixture("lightning_to_arkade", 0, TrackerOptions::default());
    track_lightning(&tracker, &id).await;
    tracker
        .hint_status(&id, crate::types::SwapStatus::ClientFunded)
        .unwrap();
    let server_funding = ActionKind::Wait {
        on: WaitingOn::ServerFunding,
    };
    assert_eq!(
        recommended(&tracker.reconcile(&id).await.unwrap()),
        server_funding
    );
    let tracker = restart(&tracker, &reader, &executor).await;
    assert_eq!(
        recommended(&tracker.reconcile(&id).await.unwrap()),
        server_funding
    );
    tracker
        .hint_status(&id, crate::types::SwapStatus::ServerWontFund)
        .unwrap();
    assert_eq!(
        recommended(&tracker.reconcile(&id).await.unwrap()),
        REFUNDED
    );
    let completed_ms = tracker.record(&id).await.unwrap().completed_ms;
    assert!(completed_ms.is_some());
    // Nothing refreshed the stored response, so it still says pending.
    assert_eq!(
        tracker
            .0
            .client
            .get_stored_swap(&id)
            .unwrap()
            .unwrap()
            .response
            .status(),
        &crate::types::SwapStatus::Pending
    );
    let tracker = restart(&tracker, &reader, &executor).await;
    let mut events = tracker.events();
    assert_eq!(
        recommended(&tracker.reconcile(&id).await.unwrap()),
        REFUNDED
    );
    assert_eq!(
        tracker.record(&id).await.unwrap().completed_ms,
        completed_ms
    );
    let event = events.try_recv().unwrap();
    assert_eq!(recommended(&event.actions.unwrap()), REFUNDED);
    assert!(events.try_recv().is_err());
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn chain_funding_overrides_a_failure_status_and_cancels_retirement() {
    let (tracker, reader, executor, id) =
        setup_fixture("lightning_to_arkade", 0, TrackerOptions::default());
    track_lightning(&tracker, &id).await;
    tracker
        .hint_status(&id, crate::types::SwapStatus::ServerWontFund)
        .unwrap();
    assert_eq!(
        recommended(&tracker.reconcile(&id).await.unwrap()),
        REFUNDED
    );
    assert!(tracker.record(&id).await.unwrap().completed_ms.is_some());
    *reader.0.lock().unwrap() = (HtlcState::Absent, HtlcState::Confirmed, 50);
    assert_eq!(
        recommended(&tracker.reconcile(&id).await.unwrap()),
        ActionKind::Wait {
            on: WaitingOn::ClaimConfirmation
        }
    );
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    let record = tracker.record(&id).await.unwrap();
    assert!(record.enabled && record.completed_ms.is_none());
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
async fn proven_not_submitted_keeps_the_funding_flag_while_a_relay_signature_can_land() {
    use crate::storage::RelayFundingOutcome;
    for (outcome, released) in [
        (RelayFundingOutcome::Rejected, false),
        (RelayFundingOutcome::Uncertain, false),
        (RelayFundingOutcome::Expired, true),
    ] {
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
        tracker
            .0
            .client
            .update_swap_metadata(
                &id,
                SwapMetadata {
                    evm_fund_started: Some(true),
                    evm_fund_relay: Some(crate::storage::RelayFundingJournal {
                        permit2_nonce: "7".into(),
                        permit2_deadline: 2_000_000_000,
                        depositor: "0x01".into(),
                        coordinator: "0x02".into(),
                        lock_token: "0x03".into(),
                        eip2612_permit: false,
                        tx_hash: None,
                        outcome,
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        tracker
            .resolve_attempt(&id, AttemptResolution::ProvenNotSubmitted)
            .await
            .unwrap();
        let metadata = stored_metadata(&tracker, &id);
        assert_eq!(
            metadata.evm_fund_started,
            Some(!released),
            "{outcome:?}: proof about a CCTP attempt says nothing about a relay signature"
        );
    }
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
#[tokio::test]
async fn swaps_stored_without_a_journal_are_adopted_after_restore() {
    use crate::arkade::StrictVhtlcOptions;
    use crate::arkade::StrictVhtlcScript;
    use bitcoin::hashes::Hash;
    use bitcoin::hashes::ripemd160;
    use bitcoin::secp256k1::Keypair;
    use bitcoin::secp256k1::Secp256k1;
    use bitcoin::secp256k1::SecretKey;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::path;
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swap-responses.json")).unwrap();
    let key = |byte: u8| {
        Keypair::from_secret_key(
            &Secp256k1::new(),
            &SecretKey::from_slice(&[byte; 32]).unwrap(),
        )
        .public_key()
    };
    let next = AtomicUsize::new(0);
    let server = MockServer::start().await;
    Mock::given(path("/swap/lightning/arkade"))
        .respond_with(move |request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let claim: bitcoin::secp256k1::PublicKey =
                body["claim_pk"].as_str().unwrap().parse().unwrap();
            let hash =
                hex::decode(body["hash_lock"].as_str().unwrap().trim_start_matches("0x")).unwrap();
            let options = StrictVhtlcOptions {
                sender: key(2).x_only_public_key().0,
                receiver: claim.x_only_public_key().0,
                server: key(3).x_only_public_key().0,
                preimage_hash: ripemd160::Hash::hash(&hash),
                refund_locktime: 4_000_000_000,
                unilateral_claim_delay: bitcoin::Sequence::from_height(21),
                unilateral_refund_delay: bitcoin::Sequence::from_height(34),
                unilateral_refund_without_receiver_delay: bitcoin::Sequence::from_height(55),
            };
            let script = StrictVhtlcScript::new(options, bitcoin::Network::Regtest).unwrap();
            let mut response = fixtures["lightning_to_arkade"].clone();
            response["id"] = json!(format!("swap-{}", next.fetch_add(1, Ordering::SeqCst)));
            response["hash_lock"] = body["hash_lock"].clone();
            response["sender_pk"] = json!(key(2).to_string());
            response["receiver_pk"] = json!(claim.to_string());
            response["arkade_server_pk"] = json!(key(3).to_string());
            response["vhtlc_refund_locktime"] = json!(4_000_000_000u32);
            response["unilateral_claim_delay"] = json!(21);
            response["unilateral_refund_delay"] = json!(34);
            response["unilateral_refund_without_receiver_delay"] = json!(55);
            response["arkade_vhtlc_address"] = json!(script.address().encode());
            ResponseTemplate::new(200).set_body_json(response)
        })
        .expect(2)
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about")
        .storage(InMemorySwapStorage::shared())
        .build()
        .unwrap();
    for _ in 0..2 {
        client
            .create_lightning_to_arkade_swap(100_000, crate::types::Address::Arkade("tark1qqellv77udfmr20tun8dvju5vgudpf9vxe8jwhthrkn26fz96pawqfdy8nk05rsmrf8h94j26905e7n6sng8y059z8ykn2j5xcuw4xt846qj6x".into()))
            .await
            .unwrap();
    }
    let reader = Arc::new(Reader(
        Mutex::new((HtlcState::Absent, HtlcState::Absent, 50)),
        AtomicBool::new(false),
    ));
    let fresh = |max_swaps| {
        SwapTracker::new(
            client.clone(),
            ContractManager::new(reader.clone(), ManagerOptions::default()).unwrap(),
            TrackerOptions {
                max_swaps,
                ..Default::default()
            },
            None,
        )
        .unwrap()
    };
    let references = crate::observation::ReferenceOptions::default();
    let tracker = fresh(1);
    assert!(tracker.restore().await.unwrap().is_empty());
    let failures = tracker
        .adopt_untracked(|_| Some((references.clone(), TrackingOptions::default())))
        .await
        .unwrap();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].1.contains("capacity"), "{failures:?}");
    assert!(tracker.record("swap-0").await.unwrap().enabled);
    let set_status = |status: crate::types::SwapStatus| {
        let mut stored = client.get_stored_swap("swap-1").unwrap().unwrap();
        let crate::types::GetSwapResponse::LightningToArkade(response) = &mut stored.response
        else {
            panic!("expected a Lightning to Arkade swap");
        };
        response.status = status;
        client.storage.store_swap(stored).unwrap();
    };
    // A settled stored status is skipped, as in the TypeScript client.
    set_status(crate::types::SwapStatus::Expired);
    let tracker = fresh(8);
    assert!(tracker.restore().await.unwrap().is_empty());
    let failures = tracker
        .adopt_untracked(|_| Some((references.clone(), TrackingOptions::default())))
        .await
        .unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    assert!(tracker.record("swap-1").await.is_err());
    set_status(crate::types::SwapStatus::Pending);
    let failures = tracker
        .adopt_untracked(|_| Some((references.clone(), TrackingOptions::default())))
        .await
        .unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    for id in ["swap-0", "swap-1"] {
        let record = tracker.record(id).await.unwrap();
        assert!(record.enabled && record.contracts.server.is_some());
    }
}
fn claim_automation(actions: &SwapActions) -> Automation {
    let action = actions.recommended().unwrap();
    assert_eq!(action.kind, ActionKind::Claim);
    action.automation
}
#[tokio::test]
async fn unsent_claims_back_off_to_the_cap_then_turn_manual_until_reconciled() {
    let (tracker, reader, executor, id) = setup(
        6,
        TrackerOptions {
            retry_delay: Duration::from_millis(20),
            retry_delay_max: Duration::from_millis(50),
            max_attempts: 4,
            ..Default::default()
        },
    );
    track(&tracker, &id).await;
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Confirmed, 50);
    let mut events = tracker.events();
    for (run, delay) in [20, 40, 50].into_iter().enumerate() {
        let start = now_ms().unwrap();
        let actions = tracker.reconcile(&id).await.unwrap();
        let end = now_ms().unwrap();
        assert_eq!(executor.calls.load(Ordering::SeqCst), run + 1);
        let AttemptState::Retryable { after_ms } =
            tracker.record(&id).await.unwrap().attempt.unwrap().state
        else {
            panic!("an unsent claim is retryable");
        };
        assert!(
            (start + delay..=end + delay).contains(&after_ms),
            "run {run}: retry at {after_ms}, expected {delay} ms after {start}..={end}"
        );
        assert_eq!(claim_automation(&actions), Automation::Automatic);
        assert!(!actions.needs_user());
        tokio::time::sleep(Duration::from_millis(delay + 5)).await;
    }
    let actions = tracker.reconcile(&id).await.unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 4);
    assert_eq!(claim_automation(&actions), Automation::Manual);
    assert!(actions.needs_user());
    tokio::time::sleep(Duration::from_millis(60)).await;
    tracker.reconcile(&id).await.unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 4);
    let mut reported = 0;
    while let Ok(event) = events.try_recv() {
        reported += usize::from(
            event
                .error
                .is_some_and(|error| error.contains("connection refused")),
        );
    }
    assert_eq!(reported, 4);
    tracker
        .resolve_attempt(&id, AttemptResolution::ProvenNotSubmitted)
        .await
        .unwrap();
    tracker.reconcile(&id).await.unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 5);
}
#[tokio::test]
async fn an_unsent_claim_is_not_run_again_before_its_deadline() {
    let (tracker, reader, executor, id) = setup(
        6,
        TrackerOptions {
            retry_delay: Duration::from_secs(60),
            retry_delay_max: Duration::from_secs(60),
            ..Default::default()
        },
    );
    track(&tracker, &id).await;
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Confirmed, 50);
    tracker.reconcile(&id).await.unwrap();
    tracker.reconcile(&id).await.unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn a_changed_recommendation_gets_a_fresh_attempt_budget() {
    let (tracker, reader, executor, id) = setup(
        6,
        TrackerOptions {
            retry_delay: Duration::from_millis(10),
            max_attempts: 2,
            ..Default::default()
        },
    );
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
    tokio::time::sleep(Duration::from_millis(30)).await;
    let actions = tracker.reconcile(&id).await.unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
    let recovery = actions.recommended().unwrap();
    assert!(matches!(recovery.kind, ActionKind::RecoverCctp { .. }));
    assert_eq!(recovery.automation, Automation::Manual);
    // The user mints by hand, and the server funds.
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Confirmed, 50);
    tracker.reconcile(&id).await.unwrap();
    assert_eq!(executor.calls.load(Ordering::SeqCst), 3);
    let attempt = tracker.record(&id).await.unwrap().attempt.unwrap();
    assert_eq!((attempt.action, attempt.count), (ActionKind::Claim, 1));
}
#[tokio::test]
async fn a_retry_runs_at_its_deadline_without_waiting_for_a_tick() {
    let (tracker, reader, executor, id) = setup(
        6,
        TrackerOptions {
            at_risk_interval: Duration::from_secs(3600),
            full_interval: Duration::from_secs(3600),
            retry_delay: Duration::from_millis(200),
            retry_delay_max: Duration::from_secs(1),
            max_attempts: 3,
            ..Default::default()
        },
    );
    track(&tracker, &id).await;
    *reader.0.lock().unwrap() = (HtlcState::Confirmed, HtlcState::Confirmed, 50);
    let worker = tracker.start().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while executor.calls.load(Ordering::SeqCst) < 3 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("each retry ran at its deadline");
    worker.shutdown().await.unwrap();
    assert!(
        tracker
            .actions(&id)
            .unwrap()
            .is_some_and(|actions| actions.needs_user())
    );
}
#[tokio::test]
async fn an_executor_must_time_out_before_the_tracker_does() {
    let (tracker, reader, _, _) = setup(0, TrackerOptions::default());
    let claimer = |timeout| {
        Arc::new(
            AutoClaimer::new(
                tracker.0.client.clone(),
                AutoClaimConfig {
                    timeout,
                    ..Default::default()
                },
            )
            .unwrap(),
        ) as Arc<dyn ActionExecutor>
    };
    let tracker_with = |executor| {
        SwapTracker::new(
            tracker.0.client.clone(),
            ContractManager::new(reader.clone(), ManagerOptions::default()).unwrap(),
            TrackerOptions::default(),
            Some(executor),
        )
    };
    assert!(tracker_with(claimer(Duration::from_secs(90))).is_ok());
    assert!(tracker_with(claimer(Duration::from_secs(120))).is_err());
}
