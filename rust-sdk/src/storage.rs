//! Per-swap state persistence.
//!
//! Records preserve the API snapshot and local execution metadata. Private keys
//! and preimages are derived from the signing seed and never persisted here.
//!
//! [`SwapStorage`] is what callers implement against their persistence
//! layer (an embedded DB, a server-side store, …). [`InMemorySwapStorage`]
//! is shipped for tests and quick starts. [`SqliteSwapStorage`] provides a
//! durable local store, configured with [`crate::ClientBuilder::storage_path`].

use crate::error::Error;
use crate::error::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

mod record;
mod sqlite;
pub use record::EvmKeySource;
pub use record::StoredSwap;
pub use record::SwapMetadata;
pub use sqlite::SqliteSwapStorage;

// BIP-32 child indices must fit in 31 bits before hardening.
const KEY_INDEX_LIMIT: u32 = 1 << 31;

/// Trait implemented by callers to persist swap state and allocate
/// monotonically-increasing key indices.
///
/// Implementations must be safe to call from multiple async tasks
/// concurrently (`Send + Sync`). The trait is synchronous on purpose —
/// callers with async persistence can wrap their store in a blocking
/// adapter rather than forcing every consumer to be async-aware.
pub trait SwapStorage: Send + Sync {
    /// Atomically allocate the next `key_index` for a new swap. The
    /// returned value is unique within this storage instance and
    /// monotonically increasing.
    ///
    /// Implementations backed by durable storage must persist the
    /// counter so that an SDK restart doesn't reuse indices.
    /// Indices must be below `2^31`; return an error on exhaustion.
    fn next_key_index(&self) -> Result<u32>;

    /// Remember the `key_index` used to derive material for the given
    /// `swap_id`. Idempotent: calling twice with the same ID replaces
    /// the value.
    fn put_swap_key_index(&self, swap_id: &str, key_index: u32) -> Result<()>;

    /// Retrieve the `key_index` previously stored for `swap_id`, if any.
    fn get_swap_key_index(&self, swap_id: &str) -> Result<Option<u32>>;

    /// Save a snapshot and merge its metadata, retaining its original index and creation time.
    /// Legacy custom stores default to index-only persistence. Override this and
    /// the record methods to support offline listing and execution resumption.
    fn store_swap(&self, record: StoredSwap) -> Result<()> {
        record.validate()?;
        self.put_swap_key_index(record.response.id(), record.key_index)
    }

    fn get_swap(&self, _swap_id: &str) -> Result<Option<StoredSwap>> {
        Err(Error::Storage(
            "this store does not support complete records".into(),
        ))
    }

    /// List records by swap ID, in ascending order.
    fn list_swaps(&self) -> Result<Vec<StoredSwap>> {
        Err(Error::Storage(
            "this store does not support complete records".into(),
        ))
    }

    /// Atomically merge supplied fields; absent fields are retained.
    fn update_metadata(&self, _swap_id: &str, _metadata: SwapMetadata) -> Result<()> {
        Err(Error::Storage(
            "this store does not support complete records".into(),
        ))
    }

    /// Remove a record and its key mapping, without resetting the allocation counter.
    fn delete_swap(&self, _swap_id: &str) -> Result<()> {
        Err(Error::Storage(
            "this store does not support deleting records".into(),
        ))
    }

    /// Remove this wallet's records and mappings, without resetting the allocation counter.
    fn clear(&self) -> Result<()> {
        Err(Error::Storage(
            "this store does not support deleting records".into(),
        ))
    }

    /// Advance the allocation counter after recovery without ever moving it back.
    /// Custom stores must implement this to support [`crate::Client::recover_swaps`].
    fn ensure_next_key_index(&self, _next_index: u32) -> Result<()> {
        Err(Error::Storage(
            "this store does not support wallet recovery".into(),
        ))
    }
}

/// Thread-safe in-memory store. Useful for tests and ephemeral processes;
/// not durable across restarts (key-index counter restarts at zero).
#[derive(Default)]
pub struct InMemorySwapStorage {
    state: Mutex<MemoryState>,
    next_index: AtomicU32,
}

#[derive(Default)]
struct MemoryState {
    indices: HashMap<String, u32>,
    records: HashMap<String, StoredSwap>,
}

impl InMemorySwapStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Wrap in `Arc` for use with [`crate::ClientBuilder::storage`].
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl SwapStorage for InMemorySwapStorage {
    fn store_swap(&self, mut record: StoredSwap) -> Result<()> {
        record.validate()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::Storage("swap storage lock poisoned".into()))?;
        let id = record.response.id().to_owned();
        if state
            .indices
            .get(&id)
            .is_some_and(|index| *index != record.key_index)
        {
            return Err(Error::Storage(
                "cannot replace a stored swap's key index".into(),
            ));
        }
        if let Some(old) = state.records.get(&id) {
            let mut merged = old.clone();
            merged.merge(record)?;
            record = merged;
        }
        self.next_index
            .fetch_max(record.key_index + 1, Ordering::Relaxed);
        state.indices.insert(id.clone(), record.key_index);
        state.records.insert(id, record);
        Ok(())
    }

    fn get_swap(&self, swap_id: &str) -> Result<Option<StoredSwap>> {
        Ok(self
            .state
            .lock()
            .map_err(|_| Error::Storage("swap storage lock poisoned".into()))?
            .records
            .get(swap_id)
            .cloned())
    }

    fn list_swaps(&self) -> Result<Vec<StoredSwap>> {
        let mut records: Vec<_> = self
            .state
            .lock()
            .map_err(|_| Error::Storage("swap storage lock poisoned".into()))?
            .records
            .values()
            .cloned()
            .collect();
        records.sort_by(|a, b| a.response.id().cmp(b.response.id()));
        Ok(records)
    }

    fn update_metadata(&self, swap_id: &str, metadata: SwapMetadata) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::Storage("swap storage lock poisoned".into()))?;
        let record = state
            .records
            .get_mut(swap_id)
            .ok_or_else(|| Error::Storage("swap record not found".into()))?;
        let now = record::now()?;
        record.metadata.merge(metadata);
        record.updated_at = now.max(record.updated_at);
        Ok(())
    }

    fn delete_swap(&self, swap_id: &str) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::Storage("swap storage lock poisoned".into()))?;
        state.records.remove(swap_id);
        state.indices.remove(swap_id);
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::Storage("swap storage lock poisoned".into()))?;
        state.records.clear();
        state.indices.clear();
        Ok(())
    }

    fn ensure_next_key_index(&self, next_index: u32) -> Result<()> {
        if next_index > KEY_INDEX_LIMIT {
            return Err(Error::Storage("next key index exceeds 2^31".into()));
        }
        self.next_index.fetch_max(next_index, Ordering::Relaxed);
        Ok(())
    }

    fn next_key_index(&self) -> Result<u32> {
        self.next_index
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |index| {
                (index < KEY_INDEX_LIMIT).then(|| index + 1)
            })
            .map_err(|_| Error::Storage("key indices exhausted".into()))
    }

    fn put_swap_key_index(&self, swap_id: &str, key_index: u32) -> Result<()> {
        if key_index >= KEY_INDEX_LIMIT {
            return Err(Error::Storage("key index must be below 2^31".into()));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::Storage("swap storage lock poisoned".into()))?;
        if state
            .records
            .get(swap_id)
            .is_some_and(|r| r.key_index != key_index)
        {
            return Err(Error::Storage(
                "cannot replace a stored swap's key index".into(),
            ));
        }
        state.indices.insert(swap_id.to_string(), key_index);
        self.next_index.fetch_max(key_index + 1, Ordering::Relaxed);
        Ok(())
    }

    fn get_swap_key_index(&self, swap_id: &str) -> Result<Option<u32>> {
        Ok(self
            .state
            .lock()
            .map_err(|_| Error::Storage("swap storage lock poisoned".into()))?
            .indices
            .get(swap_id)
            .copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_round_trips_key_index() {
        let s = InMemorySwapStorage::new();
        s.put_swap_key_index("swap_1", 7).unwrap();
        assert_eq!(s.get_swap_key_index("swap_1").unwrap(), Some(7));
        assert_eq!(s.get_swap_key_index("missing").unwrap(), None);
    }

    #[test]
    fn in_memory_overwrites_on_repeat_put() {
        let s = InMemorySwapStorage::new();
        s.put_swap_key_index("swap_1", 1).unwrap();
        s.put_swap_key_index("swap_1", 42).unwrap();
        assert_eq!(s.get_swap_key_index("swap_1").unwrap(), Some(42));
    }

    #[test]
    fn next_key_index_is_monotonic_and_unique() {
        let s = InMemorySwapStorage::new();
        let a = s.next_key_index().unwrap();
        let b = s.next_key_index().unwrap();
        let c = s.next_key_index().unwrap();
        assert_eq!(a, 0);
        assert_eq!(b, 1);
        assert_eq!(c, 2);
    }
}
