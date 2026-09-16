use super::*;
use crate::control::Cancellation;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::task::JoinSet;
use tokio::time::Instant;

#[derive(Clone, Debug)]
pub struct ManagerOptions {
    pub max_contracts: usize,
    pub concurrency: usize,
    pub event_capacity: usize,
    pub read_timeout: Duration,
    pub maximum_age: Duration,
    pub poll_interval: Duration,
}
impl Default for ManagerOptions {
    fn default() -> Self {
        Self {
            max_contracts: 1024,
            concurrency: 8,
            event_capacity: 256,
            read_timeout: Duration::from_secs(30),
            maximum_age: Duration::from_secs(60),
            poll_interval: Duration::from_secs(15),
        }
    }
}
#[derive(Clone, Debug)]
pub struct ObservationEvent {
    pub key: String,
    pub observation: Observation,
    pub error: Option<String>,
}
struct Entry {
    read: tokio::sync::Mutex<()>,
    reference: ContractRef,
    state: Mutex<Option<(Instant, Observation)>>,
}
struct Inner {
    reader: Arc<dyn ContractReader>,
    options: ManagerOptions,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    events: broadcast::Sender<ObservationEvent>,
}
#[derive(Clone)]
pub struct ContractManager(Arc<Inner>);
impl ContractManager {
    pub fn new(reader: Arc<dyn ContractReader>, options: ManagerOptions) -> Result<Self> {
        if options.max_contracts == 0
            || options.max_contracts > 65536
            || options.concurrency == 0
            || options.concurrency > 256
            || options.event_capacity == 0
            || options.event_capacity > 65536
            || options.read_timeout.is_zero()
            || options.maximum_age.is_zero()
            || options.poll_interval.is_zero()
        {
            return Err(invalid("invalid contract manager bounds"));
        }
        let (events, _) = broadcast::channel(options.event_capacity);
        Ok(Self(Arc::new(Inner {
            reader,
            options,
            entries: Mutex::new(HashMap::new()),
            events,
        })))
    }
    pub fn can_observe(&self, reference: &ContractRef) -> bool {
        self.0.reader.can_observe(reference)
    }
    pub async fn register(&self, reference: ContractRef) -> Result<()> {
        let key = reference.key()?;
        self.register_unread(reference)?;
        self.reconcile(&key).await
    }
    /// Register without network I/O. State remains Unknown until reconciled.
    pub fn register_unread(&self, reference: ContractRef) -> Result<()> {
        if !self.can_observe(&reference) {
            return Err(invalid("no configured reader for contract"));
        }
        let key = reference.key()?;
        {
            let mut entries = self
                .0
                .entries
                .lock()
                .map_err(|_| invalid("manager lock poisoned"))?;
            if let Some(existing) = entries.get(&key) {
                if existing.reference != reference {
                    return Err(invalid(
                        "contract already tracked with different confirmation or funding expectations",
                    ));
                }
            } else {
                if entries.len() >= self.0.options.max_contracts {
                    return Err(invalid("contract manager capacity reached"));
                }
                entries.insert(
                    key.clone(),
                    Arc::new(Entry {
                        read: tokio::sync::Mutex::new(()),
                        reference,
                        state: Mutex::new(None),
                    }),
                );
            }
        }
        Ok(())
    }
    pub fn unregister(&self, key: &str) -> Result<()> {
        self.0
            .entries
            .lock()
            .map_err(|_| invalid("manager lock poisoned"))?
            .remove(key);
        Ok(())
    }
    pub fn events(&self) -> broadcast::Receiver<ObservationEvent> {
        self.0.events.subscribe()
    }
    pub fn get(&self, key: &str) -> Result<Option<Observation>> {
        let entries = self
            .0
            .entries
            .lock()
            .map_err(|_| invalid("manager lock poisoned"))?;
        let Some(entry) = entries.get(key) else {
            return Ok(None);
        };
        let state = entry
            .state
            .lock()
            .map_err(|_| invalid("manager state poisoned"))?;
        Ok(Some(match state.as_ref() {
            Some((at, observation)) if at.elapsed() <= self.0.options.maximum_age => {
                observation.clone()
            }
            _ => Observation::unknown(),
        }))
    }
    pub async fn reconcile(&self, key: &str) -> Result<()> {
        let entry = self
            .0
            .entries
            .lock()
            .map_err(|_| invalid("manager lock poisoned"))?
            .get(key)
            .cloned();
        let Some(entry) = entry else {
            return Ok(());
        };
        let result = tokio::time::timeout(self.0.options.read_timeout, async {
            let _guard = entry.read.lock().await;
            self.0.reader.observe(&entry.reference).await
        })
        .await
        .map_err(|_| Error::Timeout("contract observation".into()))
        .and_then(|result| result);
        let (observation, error) = match &result {
            Ok(observation) => (observation.clone(), None),
            Err(error) => (Observation::unknown(), Some(error.to_string())),
        };
        // Unregister/re-register while a read was in flight cannot restore an old
        // registration, even when its key is identical.
        let entries = self
            .0
            .entries
            .lock()
            .map_err(|_| invalid("manager lock poisoned"))?;
        if entries
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, &entry))
        {
            let mut state = entry
                .state
                .lock()
                .map_err(|_| invalid("manager state poisoned"))?;
            let changed = state
                .as_ref()
                .is_none_or(|(_, previous)| previous != &observation);
            *state = Some((Instant::now(), observation.clone()));
            if changed || error.is_some() {
                let _ = self.0.events.send(ObservationEvent {
                    key: key.to_string(),
                    observation,
                    error,
                });
            }
        }
        result.map(|_| ())
    }
    /// Refresh all contracts with bounded concurrency. One failed reader does
    /// not suppress other chains; each error is emitted and its state invalidated.
    pub async fn refresh(&self) -> Result<()> {
        let keys: Vec<_> = self
            .0
            .entries
            .lock()
            .map_err(|_| invalid("manager lock poisoned"))?
            .keys()
            .cloned()
            .collect();
        let mut keys = keys.into_iter();
        let mut tasks = JoinSet::new();
        loop {
            while tasks.len() < self.0.options.concurrency {
                let Some(key) = keys.next() else {
                    break;
                };
                let manager = self.clone();
                tasks.spawn(async move {
                    let _ = manager.reconcile(&key).await;
                });
            }
            match tasks.join_next().await {
                Some(Ok(())) => {}
                Some(Err(error)) => return Err(Error::Transport(error.to_string())),
                None => return Ok(()),
            }
        }
    }
    pub fn start(&self) -> Result<ObservationMonitor> {
        tokio::runtime::Handle::try_current()
            .map_err(|_| invalid("manager requires Tokio runtime"))?;
        let manager = self.clone();
        let cancellation = Cancellation::default();
        let stop = cancellation.clone();
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(manager.0.options.poll_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {biased;_=stop.cancelled()=>break,_=async {interval.tick().await;let _=manager.refresh().await;}=>{}}
            }
        });
        Ok(ObservationMonitor {
            cancellation,
            task: Some(task),
        })
    }
}
pub struct ObservationMonitor {
    cancellation: Cancellation,
    task: Option<JoinHandle<()>>,
}
impl ObservationMonitor {
    pub async fn shutdown(mut self) -> Result<()> {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.await.map_err(|e| Error::Transport(e.to_string()))?;
        }
        Ok(())
    }
}
impl Drop for ObservationMonitor {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
