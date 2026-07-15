//! Per-swap state persistence.
//!
//! Each swap the SDK creates owns a `key_index` — the integer the
//! [`crate::Signer`] uses to derive the swap's signing key, EVM key, and
//! claim preimage. Because every piece of secret material is
//! deterministically re-derivable from `(mnemonic / xprv, key_index)`,
//! that small integer is the only thing the SDK has to remember per
//! swap. Persisting it lets the client survive process restarts and
//! still claim the swap later.
//!
//! [`SwapStorage`] is what callers implement against their persistence
//! layer (an embedded DB, a server-side store, …). [`InMemorySwapStorage`]
//! is shipped for tests and quick starts; production callers should swap
//! in something durable.

use crate::error::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

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
    fn next_key_index(&self) -> Result<u32>;

    /// Remember the `key_index` used to derive material for the given
    /// `swap_id`. Idempotent: calling twice with the same ID replaces
    /// the value.
    ///
    /// Knowing `(mnemonic_or_xprv, key_index)` is sufficient to
    /// re-derive every secret the SDK needs to claim or refund the
    /// swap, so this is the only durable bit of state per swap.
    fn put_swap_key_index(&self, swap_id: &str, key_index: u32) -> Result<()>;

    /// Retrieve the `key_index` previously stored for `swap_id`, if any.
    fn get_swap_key_index(&self, swap_id: &str) -> Result<Option<u32>>;
}

/// Thread-safe in-memory store. Useful for tests and ephemeral processes;
/// not durable across restarts (key-index counter restarts at zero).
#[derive(Default)]
pub struct InMemorySwapStorage {
    swaps: Mutex<HashMap<String, u32>>,
    next_index: AtomicU32,
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
    fn next_key_index(&self) -> Result<u32> {
        Ok(self.next_index.fetch_add(1, Ordering::Relaxed))
    }

    fn put_swap_key_index(&self, swap_id: &str, key_index: u32) -> Result<()> {
        self.swaps
            .lock()
            .expect("SwapStorage mutex poisoned")
            .insert(swap_id.to_string(), key_index);
        Ok(())
    }

    fn get_swap_key_index(&self, swap_id: &str) -> Result<Option<u32>> {
        Ok(self
            .swaps
            .lock()
            .expect("SwapStorage mutex poisoned")
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
