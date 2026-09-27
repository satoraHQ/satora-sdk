//! Persistent, chain-driven swap tracking and opt-in automation.
//!
//! Use one tracker/execution owner per store. Callbacks must use the SDK's durable
//! execution methods. Errors, timeouts and interrupted callbacks are ambiguous
//! submissions: only explicit reconciliation can enable another attempt.
use crate::Client;
use crate::Error;
use crate::Result;
use crate::SwapMetadata;
use crate::actions::*;
use crate::control::Cancellation;
use crate::observation::ContractManager;
use crate::observation::SwapContracts;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::sync::Notify;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::task::JoinSet;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TrackingOptions {
    pub auto_claim: bool,
    pub auto_cctp: bool,
    /// Application-verified Arkade capability; ignored for other client rails.
    pub collaborative_refund_available: bool,
    /// Feed verified bridge receipts/attestations/delivery results here.
    pub bridge: BridgeState,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AttemptState {
    Running,
    Submitted { reference: String },
    Retryable { after_ms: u64 },
    Uncertain,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attempt {
    pub action: ActionKind,
    pub count: u32,
    pub state: AttemptState,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrackingRecord {
    pub version: u32,
    pub enabled: bool,
    pub contracts: SwapContracts,
    pub options: TrackingOptions,
    pub attempt: Option<Attempt>,
    /// Wall time a terminal outcome was first seen. Any reconcile that does not
    /// derive a terminal outcome clears it, including one whose read failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_ms: Option<u64>,
}
#[derive(Clone, Debug)]
pub struct TrackerOptions {
    pub max_swaps: usize,
    pub concurrency: usize,
    pub event_capacity: usize,
    pub at_risk_interval: Duration,
    pub full_interval: Duration,
    pub execution_timeout: Duration,
    pub retry_delay: Duration,
    pub max_attempts: u32,
    pub policy: ActionPolicy,
    /// How long a terminal outcome must hold before the swap is untracked. It
    /// stands in for reorg depth; zero retires on the first terminal reconcile.
    pub retire_after: Duration,
}
impl Default for TrackerOptions {
    fn default() -> Self {
        Self {
            max_swaps: 512,
            concurrency: 8,
            event_capacity: 256,
            at_risk_interval: Duration::from_secs(5),
            full_interval: Duration::from_secs(60),
            execution_timeout: Duration::from_secs(120),
            retry_delay: Duration::from_secs(30),
            max_attempts: 3,
            policy: ActionPolicy::default(),
            retire_after: Duration::from_secs(86_400),
        }
    }
}
#[derive(Clone, Debug)]
pub struct TrackerEvent {
    pub swap_id: String,
    pub actions: Option<SwapActions>,
    pub error: Option<String>,
}
/// Returning `Retryable` asserts that no transaction or user operation escaped.
/// All ordinary errors are treated as uncertain, including transport failures.
pub enum ExecutionResult {
    Submitted { reference: String },
    Retryable,
    Uncertain,
}
pub type ExecutionFuture<'a> = Pin<Box<dyn Future<Output = Result<ExecutionResult>> + Send + 'a>>;
pub trait ActionExecutor: Send + Sync {
    fn execute<'a>(&'a self, swap_id: &'a str, action: &'a ActionKind) -> ExecutionFuture<'a>;
}
/// Application evidence obtained by inspecting receipts, account nonces, and the
/// underlying execution journal.
pub enum AttemptResolution {
    /// Proof clears the tracker attempt and releases only that action's unsent
    /// reservations: the EVM claim flag for Claim; for RecoverCctp, hashless
    /// cctp_fund/cctp_mint operations and a hashless funding flag, which a
    /// wallet-mode executor sets through `fund_swap_with_wallet`. Refund and
    /// recovery reservations are never released here. The proof must also cover
    /// any funding sent outside the tracker since the attempt started.
    ProvenNotSubmitted,
    Submitted {
        reference: String,
    },
}
struct Entry {
    record: tokio::sync::Mutex<TrackingRecord>,
    actions: Mutex<Option<SwapActions>>,
}
struct Inner {
    client: Client,
    manager: ContractManager,
    options: TrackerOptions,
    executor: Option<Arc<dyn ActionExecutor>>,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    registration: tokio::sync::Mutex<()>,
    pending: Mutex<HashSet<String>>,
    wake: Notify,
    events: broadcast::Sender<TrackerEvent>,
}
#[derive(Clone)]
pub struct SwapTracker(Arc<Inner>);
fn invalid(message: &str) -> Error {
    Error::InvalidSwap(message.into())
}
fn now_ms() -> Result<u64> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| Error::Storage(e.to_string()))?
            .as_millis(),
    )
    .map_err(|e| Error::Storage(e.to_string()))
}
impl SwapTracker {
    pub fn new(
        client: Client,
        manager: ContractManager,
        options: TrackerOptions,
        executor: Option<Arc<dyn ActionExecutor>>,
    ) -> Result<Self> {
        if options.max_swaps == 0
            || options.max_swaps > 32768
            || options.concurrency == 0
            || options.concurrency > 256
            || options.event_capacity == 0
            || options.event_capacity > 65536
            || options.at_risk_interval.is_zero()
            || options.full_interval.is_zero()
            || options.execution_timeout.is_zero()
            || options.retry_delay.is_zero()
            || options.max_attempts == 0
        {
            return Err(invalid("invalid tracker bounds"));
        }
        let (events, _) = broadcast::channel(options.event_capacity);
        Ok(Self(Arc::new(Inner {
            client,
            manager,
            options,
            executor,
            entries: Mutex::new(HashMap::new()),
            registration: tokio::sync::Mutex::new(()),
            pending: Mutex::new(HashSet::new()),
            wake: Notify::new(),
            events,
        })))
    }
    fn entry(&self, id: &str) -> Result<Arc<Entry>> {
        self.0
            .entries
            .lock()
            .map_err(|_| invalid("tracker lock poisoned"))?
            .get(id)
            .cloned()
            .ok_or_else(|| invalid("swap is not tracked"))
    }
    fn save(&self, id: &str, record: &TrackingRecord) -> Result<()> {
        self.0.client.update_swap_metadata(
            id,
            SwapMetadata {
                tracking: Some(
                    serde_json::to_value(record).map_err(|e| Error::Storage(e.to_string()))?,
                ),
                ..Default::default()
            },
        )
    }
    pub fn events(&self) -> broadcast::Receiver<TrackerEvent> {
        self.0.events.subscribe()
    }
    pub fn actions(&self, id: &str) -> Result<Option<SwapActions>> {
        Ok(self
            .entry(id)?
            .actions
            .lock()
            .map_err(|_| invalid("tracker state poisoned"))?
            .clone())
    }
    pub async fn record(&self, id: &str) -> Result<TrackingRecord> {
        Ok(self.entry(id)?.record.lock().await.clone())
    }
    /// The references should come from `Client::contract_references`, which checks
    /// scripts and ownership against the stored swap. Custom references are trusted.
    pub async fn track(
        &self,
        id: &str,
        contracts: SwapContracts,
        options: TrackingOptions,
    ) -> Result<()> {
        let _guard = self.0.registration.lock().await;
        if self.0.client.get_stored_swap(id)?.is_none() {
            return Err(invalid("swap record missing"));
        }
        if self
            .0
            .client
            .get_stored_swap(id)?
            .and_then(|r| r.metadata.tracking)
            .is_some()
        {
            return Err(invalid(
                "tracking journal already exists; restore it and update options",
            ));
        }
        self.install(
            id,
            TrackingRecord {
                version: 1,
                enabled: true,
                contracts,
                options,
                attempt: None,
                completed_ms: None,
            },
        )
        .await
    }
    async fn install(&self, id: &str, record: TrackingRecord) -> Result<()> {
        if record.version != 1
            || (record.contracts.client.is_none() && record.contracts.server.is_none())
        {
            return Err(invalid("unsupported or empty tracking record"));
        }
        {
            let entries = self
                .0
                .entries
                .lock()
                .map_err(|_| invalid("tracker lock poisoned"))?;
            if entries.contains_key(id) {
                return Ok(());
            }
            if entries.len() >= self.0.options.max_swaps {
                return Err(invalid("tracker capacity reached"));
            }
        }
        for reference in record
            .contracts
            .client
            .iter()
            .chain(record.contracts.server.iter())
        {
            reference.key()?;
            if !self.0.manager.can_observe(reference) {
                return Err(invalid("no reader for tracked contract"));
            }
        }
        let mut added = Vec::new();
        let result = (|| {
            for reference in record
                .contracts
                .client
                .iter()
                .chain(record.contracts.server.iter())
            {
                let key = reference.key()?;
                let exists = self.0.manager.get(&key)?.is_some();
                self.0.manager.register_unread(reference.clone())?;
                if !exists {
                    added.push(key);
                }
            }
            self.save(id, &record)
        })();
        if let Err(error) = result {
            for key in added {
                self.0.manager.unregister(&key)?;
            }
            return Err(error);
        }
        self.0
            .entries
            .lock()
            .map_err(|_| invalid("tracker lock poisoned"))?
            .insert(
                id.into(),
                Arc::new(Entry {
                    record: tokio::sync::Mutex::new(record),
                    actions: Mutex::new(None),
                }),
            );
        self.hint(id)
    }
    /// Restore enabled records, retaining ambiguous in-flight attempts. One bad
    /// record does not prevent restoration of other swaps; failures are returned.
    pub async fn restore(&self) -> Result<Vec<(String, String)>> {
        let _guard = self.0.registration.lock().await;
        let mut failures = Vec::new();
        for stored in self.0.client.list_all_swaps()? {
            let Some(value) = stored.metadata.tracking else {
                continue;
            };
            let id = stored.response.id().to_string();
            let result = async {
                let record: TrackingRecord =
                    serde_json::from_value(value).map_err(|e| Error::Decode(e.to_string()))?;
                if record.enabled {
                    self.install(&id, record).await?;
                }
                Ok::<_, Error>(())
            }
            .await;
            if let Err(error) = result {
                failures.push((id, error.to_string()));
            }
        }
        failures.extend(self.0.client.unreadable_swaps()?);
        Ok(failures)
    }
    /// Disable tracking while retaining the execution journal for later recovery.
    pub async fn untrack(&self, id: &str) -> Result<()> {
        self.disable(id, false).await
    }
    fn settled(&self, record: &TrackingRecord) -> Result<bool> {
        let now = now_ms()?;
        let after = u64::try_from(self.0.options.retire_after.as_millis()).unwrap_or(u64::MAX);
        Ok(record.enabled
            && record
                .completed_ms
                .is_some_and(|since| now >= since.saturating_add(after)))
    }
    async fn disable(&self, id: &str, only_if_settled: bool) -> Result<()> {
        let _guard = self.0.registration.lock().await;
        let entry = match self.entry(id) {
            Ok(entry) => entry,
            Err(_) if only_if_settled => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut record = entry.record.lock().await;
        // A later reconcile that cleared completed_ms (a reorg or a failed read), or an
        // untrack in between, cancels retirement.
        if only_if_settled && !self.settled(&record)? {
            return Ok(());
        }
        let mut next = record.clone();
        next.enabled = false;
        self.save(id, &next)?;
        *record = next;
        self.0
            .entries
            .lock()
            .map_err(|_| invalid("tracker lock poisoned"))?
            .remove(id);
        self.0
            .pending
            .lock()
            .map_err(|_| invalid("tracker queue poisoned"))?
            .remove(id);
        let entries: Vec<_> = self
            .0
            .entries
            .lock()
            .map_err(|_| invalid("tracker lock poisoned"))?
            .values()
            .cloned()
            .collect();
        let mut used = HashSet::new();
        for other in entries {
            let other = other.record.lock().await;
            for reference in other
                .contracts
                .client
                .iter()
                .chain(other.contracts.server.iter())
            {
                used.insert(reference.key()?);
            }
        }
        for reference in record
            .contracts
            .client
            .iter()
            .chain(record.contracts.server.iter())
        {
            let key = reference.key()?;
            if !used.contains(&key) {
                self.0.manager.unregister(&key)?;
            }
        }
        Ok(())
    }
    /// Re-enable a disabled journal without losing submission history.
    pub async fn resume(&self, id: &str) -> Result<()> {
        let _guard = self.0.registration.lock().await;
        let value = self
            .0
            .client
            .get_stored_swap(id)?
            .and_then(|r| r.metadata.tracking)
            .ok_or_else(|| invalid("tracking journal missing"))?;
        let mut record: TrackingRecord =
            serde_json::from_value(value).map_err(|e| Error::Decode(e.to_string()))?;
        record.enabled = true;
        // Otherwise a retired swap would retire again on its next reconcile.
        record.completed_ms = None;
        self.install(id, record).await
    }
    pub async fn update_options(&self, id: &str, options: TrackingOptions) -> Result<()> {
        let entry = self.entry(id)?;
        let mut record = entry.record.lock().await;
        if !record.enabled {
            return Err(invalid("tracking disabled"));
        }
        let mut next = record.clone();
        next.options = options;
        self.save(id, &next)?;
        *record = next;
        self.hint(id)
    }
    pub async fn resolve_attempt(&self, id: &str, resolution: AttemptResolution) -> Result<()> {
        let entry = self.entry(id)?;
        let mut record = entry.record.lock().await;
        let mut next = record.clone();
        match resolution {
            AttemptResolution::ProvenNotSubmitted => {
                let attempt = next
                    .attempt
                    .take()
                    .ok_or_else(|| invalid("no attempt to reconcile"))?;
                #[cfg(feature = "gasless")]
                let _execution = self.0.client.evm_execution.lock().await;
                let metadata = self
                    .0
                    .client
                    .get_stored_swap(id)?
                    .ok_or_else(|| invalid("swap record missing"))?
                    .metadata;
                let mut update = SwapMetadata::default();
                if attempt.action == ActionKind::Claim {
                    update.evm_claim_started = (metadata.evm_claim_started == Some(true)
                        && metadata.evm_claim_txid.is_none()
                        && metadata.evm_claim_userop_hash.is_none())
                    .then_some(false);
                } else if matches!(attempt.action, ActionKind::RecoverCctp { .. }) {
                    let mut operations = metadata.bridge_operations.unwrap_or_default();
                    let before = operations.len();
                    operations.retain(|key, operation| {
                        !(matches!(key.as_str(), "cctp_fund" | "cctp_mint")
                            && !operation.complete
                            && operation.transaction_hash.is_none()
                            && operation.user_operation_hash.is_none())
                    });
                    update.evm_fund_started = (metadata.evm_fund_started == Some(true)
                        && metadata.evm_fund_txid.is_none()
                        && metadata.evm_fund_userop_hash.is_none()
                        && !operations.contains_key("cctp_fund"))
                    .then_some(false);
                    update.bridge_operations = (operations.len() != before).then_some(operations);
                }
                self.0.client.update_swap_metadata(id, update)?;
            }
            AttemptResolution::Submitted { reference } => {
                if reference.is_empty() {
                    return Err(invalid("empty submission reference"));
                }
                next.attempt
                    .as_mut()
                    .ok_or_else(|| invalid("no attempt to reconcile"))?
                    .state = AttemptState::Submitted { reference };
            }
        }
        self.save(id, &next)?;
        *record = next;
        self.hint(id)
    }
    /// Coalesced, bounded hints; API status values are deliberately ignored.
    pub fn hint(&self, id: &str) -> Result<()> {
        let entries = self
            .0
            .entries
            .lock()
            .map_err(|_| invalid("tracker lock poisoned"))?;
        if entries.contains_key(id) {
            self.0
                .pending
                .lock()
                .map_err(|_| invalid("tracker queue poisoned"))?
                .insert(id.into());
            self.0.wake.notify_one();
        }
        Ok(())
    }
    pub fn hint_all(&self) -> Result<()> {
        let entries = self
            .0
            .entries
            .lock()
            .map_err(|_| invalid("tracker lock poisoned"))?;
        self.0
            .pending
            .lock()
            .map_err(|_| invalid("tracker queue poisoned"))?
            .extend(entries.keys().cloned());
        self.0.wake.notify_one();
        Ok(())
    }
    #[cfg(feature = "websocket")]
    pub fn status_hint(&self, event: &crate::subscription::StatusEvent) -> Result<()> {
        use crate::subscription::StatusEvent::*;
        match event {
            Update { swap_id, .. } | Missing { swap_id } => self.hint(swap_id),
            Connected { .. } | Lagged { .. } | Reconciled => self.hint_all(),
            _ => Ok(()),
        }
    }
    fn emit(&self, id: &str, actions: Option<SwapActions>, error: Option<String>) {
        let _ = self.0.events.send(TrackerEvent {
            swap_id: id.into(),
            actions,
            error,
        });
    }
    async fn derive(&self, id: &str, record: &TrackingRecord) -> Result<SwapActions> {
        for reference in record
            .contracts
            .client
            .iter()
            .chain(record.contracts.server.iter())
        {
            if let Err(error) = self.0.manager.reconcile(&reference.key()?).await {
                self.emit(id, None, Some(error.to_string()));
            }
        }
        let read =
            |reference: &crate::observation::ContractRef| self.0.manager.get(&reference.key()?);
        let client = record
            .contracts
            .client
            .as_ref()
            .map(read)
            .transpose()?
            .flatten();
        let server = record
            .contracts
            .server
            .as_ref()
            .map(read)
            .transpose()?
            .flatten();
        let metadata = self
            .0
            .client
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?
            .metadata;
        let attempt = record.attempt.as_ref();
        let submitted = attempt.filter(|a| matches!(a.state, AttemptState::Submitted { .. }));
        let uncertain = attempt.is_some_and(|a| {
            (matches!(a.state, AttemptState::Running | AttemptState::Uncertain)
                || matches!(a.state, AttemptState::Retryable { .. })
                    && a.count >= self.0.options.max_attempts)
        });
        let actions = derive_actions(
            ActionInput {
                contracts: &record.contracts,
                client: client.as_ref(),
                server: server.as_ref(),
                funding_submitted: metadata.evm_fund_txid.is_some()
                    || metadata.evm_fund_userop_hash.is_some()
                    || submitted
                        .is_some_and(|a| matches!(a.action, ActionKind::RecoverCctp { .. })),
                claim_submitted: metadata.evm_claim_txid.is_some()
                    || metadata.evm_claim_userop_hash.is_some()
                    || submitted.is_some_and(|a| a.action == ActionKind::Claim),
                funding_uncertain: metadata.evm_fund_started == Some(true)
                    && metadata.evm_fund_txid.is_none()
                    && metadata.evm_fund_userop_hash.is_none(),
                submission_uncertain: uncertain
                    || metadata.evm_refund_started == Some(true)
                    || metadata.btc_spend_txid.is_some()
                    || metadata.arkade_pending.is_some()
                    || metadata.arkade_delegate.is_some()
                    || (metadata.evm_claim_started == Some(true)
                        && metadata.evm_claim_txid.is_none()
                        && metadata.evm_claim_userop_hash.is_none()),
                collaborative_refund_available: record.options.collaborative_refund_available,
                bridge: &record.options.bridge,
            },
            &self.0.options.policy,
        );
        Ok(actions)
    }
    /// Refresh both legs immediately, then optionally execute the derived action.
    /// Per-swap locks serialize concurrent ticks and persist before any callback.
    pub async fn reconcile(&self, id: &str) -> Result<SwapActions> {
        let entry = self.entry(id)?;
        let mut record = entry.record.lock().await;
        if !record.enabled {
            return Err(invalid("tracking disabled"));
        }
        let mut actions = self.derive(id, &record).await?;
        if let Some(executor) = &self.0.executor {
            let kind = actions
                .recommended()
                .filter(|a| a.automation == Automation::Automatic && !a.blocked_by_timelock)
                .map(|a| a.kind.clone());
            if let Some(kind) = kind.filter(|kind| match kind {
                ActionKind::Claim => record.options.auto_claim,
                ActionKind::RecoverCctp { .. } => record.options.auto_cctp,
                _ => false,
            }) {
                let now = now_ms()?;
                let count = match &record.attempt {
                    None => Some(1),
                    Some(attempt)
                        if attempt.action == kind
                            && attempt.count < self.0.options.max_attempts =>
                    {
                        match attempt.state {
                            AttemptState::Retryable { after_ms } if now >= after_ms => {
                                Some(attempt.count + 1)
                            }
                            _ => None,
                        }
                    }
                    // A completed CCTP funding attempt can be followed by a claim.
                    Some(attempt)
                        if matches!(attempt.action, ActionKind::RecoverCctp { .. })
                            && kind == ActionKind::Claim
                            && matches!(attempt.state, AttemptState::Submitted { .. }) =>
                    {
                        Some(1)
                    }
                    _ => None,
                };
                if let Some(count) = count {
                    let mut next = record.clone();
                    next.attempt = Some(Attempt {
                        action: kind.clone(),
                        count,
                        state: AttemptState::Running,
                    });
                    self.save(id, &next)?;
                    *record = next;
                    let result = tokio::time::timeout(
                        self.0.options.execution_timeout,
                        executor.execute(id, &kind),
                    )
                    .await;
                    let state = match result {
                        Ok(Ok(ExecutionResult::Submitted { reference }))
                            if !reference.is_empty() =>
                        {
                            AttemptState::Submitted { reference }
                        }
                        Ok(Ok(ExecutionResult::Retryable)) => AttemptState::Retryable {
                            after_ms: now_ms()?.saturating_add(
                                u64::try_from(self.0.options.retry_delay.as_millis())
                                    .unwrap_or(u64::MAX),
                            ),
                        },
                        Ok(Err(error)) => {
                            self.emit(id, None, Some(error.to_string()));
                            AttemptState::Uncertain
                        }
                        Err(_) => {
                            self.emit(
                                id,
                                None,
                                Some("automatic execution timed out; reconcile submission".into()),
                            );
                            AttemptState::Uncertain
                        }
                        _ => AttemptState::Uncertain,
                    };
                    let mut next = record.clone();
                    next.attempt.as_mut().unwrap().state = state;
                    self.save(id, &next)?;
                    *record = next;
                    // A callback result alone never establishes completion.
                    actions = self.derive(id, &record).await?;
                }
            }
        }
        let terminal = match actions.recommended().map(|a| &a.kind) {
            // The burned USDC of an expired CCTP-funded swap still needs a manual mint.
            Some(ActionKind::Complete {
                outcome: Outcome::Expired,
            }) => !matches!(record.options.bridge, BridgeState::CctpBurn { .. }),
            Some(ActionKind::Complete { .. }) => true,
            _ => false,
        };
        let completed_ms = if terminal {
            Some(record.completed_ms.map_or_else(now_ms, Ok)?)
        } else {
            None
        };
        if completed_ms != record.completed_ms {
            let mut next = record.clone();
            next.completed_ms = completed_ms;
            self.save(id, &next)?;
            *record = next;
        }
        let retire = self.settled(&record)?;
        {
            let mut previous = entry
                .actions
                .lock()
                .map_err(|_| invalid("tracker state poisoned"))?;
            if previous.as_ref() != Some(&actions) {
                self.emit(id, Some(actions.clone()), None);
            }
            *previous = Some(actions.clone());
        }
        // disable takes the registration lock first, so the record lock must go.
        drop(record);
        if retire {
            self.disable(id, true).await?;
        }
        Ok(actions)
    }
    async fn refresh_ids(&self, ids: Vec<String>) -> Result<()> {
        let mut ids = ids.into_iter();
        let mut tasks = JoinSet::new();
        loop {
            while tasks.len() < self.0.options.concurrency {
                let Some(id) = ids.next() else {
                    break;
                };
                let tracker = self.clone();
                tasks.spawn(async move {
                    if let Err(error) = tracker.reconcile(&id).await {
                        tracker.emit(&id, None, Some(error.to_string()));
                    }
                });
            }
            match tasks.join_next().await {
                Some(Ok(())) => {}
                Some(Err(e)) => return Err(Error::Transport(e.to_string())),
                None => return Ok(()),
            }
        }
    }
    pub async fn refresh(&self, at_risk_only: bool) -> Result<()> {
        let ids = self
            .0
            .entries
            .lock()
            .map_err(|_| invalid("tracker lock poisoned"))?
            .iter()
            .filter(|(_, entry)| {
                !at_risk_only
                    || entry
                        .actions
                        .lock()
                        .map_or(true, |a| a.as_ref().is_none_or(|a| a.at_risk))
            })
            .map(|(id, _)| id.clone())
            .collect();
        self.refresh_ids(ids).await
    }
    pub fn start(&self) -> Result<TrackerWorker> {
        tokio::runtime::Handle::try_current()
            .map_err(|_| invalid("tracker requires Tokio runtime"))?;
        let cancellation = Cancellation::default();
        let stop = cancellation.clone();
        let tracker = self.clone();
        let task = tokio::spawn(async move {
            let mut risk = tokio::time::interval(tracker.0.options.at_risk_interval);
            let mut full = tokio::time::interval(tracker.0.options.full_interval);
            risk.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            full.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let work = async {
                    tokio::select! {
                        _ = full.tick() => tracker.refresh(false).await,
                        _ = risk.tick() => tracker.refresh(true).await,
                        _ = tracker.0.wake.notified() => {
                            let ids = tracker.0.pending.lock().map_err(|_| invalid("tracker queue poisoned"))?.drain().collect();
                            tracker.refresh_ids(ids).await
                        }
                    }
                };
                tokio::select! { biased; _ = stop.cancelled() => break, result = work => {
                    if let Err(error) = result { tracker.emit("", None, Some(error.to_string())); }
                } }
            }
        });
        Ok(TrackerWorker {
            cancellation,
            task: Some(task),
        })
    }
}
/// Drop cancels in-flight reads/callbacks. A callback interrupted after reservation
/// remains Running in storage and must be reconciled before execution can resume.
pub struct TrackerWorker {
    cancellation: Cancellation,
    task: Option<JoinHandle<()>>,
}
impl TrackerWorker {
    pub async fn shutdown(mut self) -> Result<()> {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.await.map_err(|e| Error::Transport(e.to_string()))?;
        }
        Ok(())
    }
}
impl Drop for TrackerWorker {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
#[cfg(test)]
mod tests;
