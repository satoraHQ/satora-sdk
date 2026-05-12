//! Persistence for swap secrets.
//!
//! The SDK generates a 32-byte secret per swap, hashes it (`SHA256`) into
//! the `hash_lock` that the backend sees, and keeps the secret around for
//! the eventual claim. That secret has to survive process restarts —
//! whoever holds it controls the swap.
//!
//! [`SwapStorage`] is the trait callers implement against their persistence
//! layer (an embedded DB, a server-side store, …). [`InMemorySwapStorage`]
//! is shipped for tests and quick starts; production callers should swap in
//! something durable.

use crate::error::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

/// Trait implemented by callers to persist per-swap secrets.
///
/// Implementations must be safe to call from multiple async tasks
/// concurrently (`Send + Sync`). The trait is synchronous on purpose —
/// callers with async persistence can wrap their store in a blocking
/// adapter rather than forcing every consumer to be async-aware.
pub trait SwapStorage: Send + Sync {
    /// Persist the secret for the given swap ID. Idempotent: calling twice
    /// with the same ID replaces the value.
    fn put_secret(&self, swap_id: &str, secret: &[u8]) -> Result<()>;

    /// Retrieve the secret previously stored for the given swap ID, if any.
    fn get_secret(&self, swap_id: &str) -> Result<Option<Vec<u8>>>;
}

/// Thread-safe in-memory store. Useful for tests and ephemeral processes;
/// not durable across restarts.
#[derive(Default, Clone)]
pub struct InMemorySwapStorage {
    inner: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl InMemorySwapStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SwapStorage for InMemorySwapStorage {
    fn put_secret(&self, swap_id: &str, secret: &[u8]) -> Result<()> {
        self.inner
            .lock()
            .expect("SwapStorage mutex poisoned")
            .insert(swap_id.to_string(), secret.to_vec());
        Ok(())
    }

    fn get_secret(&self, swap_id: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .inner
            .lock()
            .expect("SwapStorage mutex poisoned")
            .get(swap_id)
            .cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_round_trips_secret() {
        let s = InMemorySwapStorage::new();
        s.put_secret("swap_1", &[1, 2, 3]).unwrap();
        assert_eq!(s.get_secret("swap_1").unwrap(), Some(vec![1, 2, 3]));
        assert_eq!(s.get_secret("missing").unwrap(), None);
    }

    #[test]
    fn in_memory_overwrites_on_repeat_put() {
        let s = InMemorySwapStorage::new();
        s.put_secret("swap_1", &[1]).unwrap();
        s.put_secret("swap_1", &[9, 9, 9]).unwrap();
        assert_eq!(s.get_secret("swap_1").unwrap(), Some(vec![9, 9, 9]));
    }
}
