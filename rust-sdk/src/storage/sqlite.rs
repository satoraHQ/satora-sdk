use super::KEY_INDEX_LIMIT;
use super::StoredSwap;
use super::SwapMetadata;
use super::SwapStorage;
use crate::Error;
use crate::Result;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::TransactionBehavior;
use rusqlite::params;
use std::path::Path;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::Duration;

/// Durable swap key indices and allocation counter, partitioned by wallet.
///
/// Stores no private keys or preimages. Back up this database alongside the
/// signing seed to resume funding and claiming after a restart. Multiple
/// clients/processes may share a file; allocation uses SQLite transactions.
pub struct SqliteSwapStorage {
    connection: Mutex<Connection>,
    wallet_id: String,
}

impl SqliteSwapStorage {
    /// Open or create a database in an existing directory.
    ///
    /// `wallet_id` must be stable and unique to the signing seed. Prefer
    /// [`crate::ClientBuilder::storage_path`], which derives it automatically.
    /// Use a local filesystem with SQLite locking support.
    pub fn open(path: impl AsRef<Path>, wallet_id: &str) -> Result<Self> {
        if wallet_id.is_empty() {
            return Err(Error::Storage("wallet ID must not be empty".into()));
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS sdk_key_indices (
                wallet_id TEXT PRIMARY KEY NOT NULL,
                next_index INTEGER NOT NULL CHECK(next_index BETWEEN 0 AND 2147483648)
            );
            CREATE TABLE IF NOT EXISTS sdk_swap_keys (
                wallet_id TEXT NOT NULL,
                swap_id TEXT NOT NULL,
                key_index INTEGER NOT NULL CHECK(key_index BETWEEN 0 AND 2147483647),
                PRIMARY KEY(wallet_id, swap_id)
            );
            CREATE TABLE IF NOT EXISTS sdk_swap_records (
                wallet_id TEXT NOT NULL,
                swap_id TEXT NOT NULL,
                record_json TEXT NOT NULL,
                PRIMARY KEY(wallet_id, swap_id)
            );",
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO sdk_key_indices(wallet_id, next_index) VALUES (?1, 0)",
            [wallet_id],
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
            wallet_id: wallet_id.to_owned(),
        })
    }

    fn read_record(
        connection: &Connection,
        wallet_id: &str,
        swap_id: &str,
    ) -> Result<Option<StoredSwap>> {
        let json: Option<String> = connection
            .query_row(
                "SELECT record_json FROM sdk_swap_records WHERE wallet_id = ?1 AND swap_id = ?2",
                params![wallet_id, swap_id],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|json| {
            let record: StoredSwap =
                serde_json::from_str(&json).map_err(|e| Error::Storage(e.to_string()))?;
            record.validate()?;
            if record.response.id() != swap_id {
                return Err(Error::Storage(
                    "swap record ID does not match its database key".into(),
                ));
            }
            Ok(record)
        })
        .transpose()
    }

    fn write_record(connection: &Connection, wallet_id: &str, record: &StoredSwap) -> Result<()> {
        let json = serde_json::to_string(record).map_err(|e| Error::Storage(e.to_string()))?;
        connection.execute(
            "INSERT INTO sdk_swap_records(wallet_id, swap_id, record_json) VALUES (?1, ?2, ?3)
             ON CONFLICT(wallet_id, swap_id) DO UPDATE SET record_json = excluded.record_json",
            params![wallet_id, record.response.id(), json],
        )?;
        Ok(())
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| Error::Storage("swap storage lock poisoned".into()))
    }
}

impl SwapStorage for SqliteSwapStorage {
    fn store_swap(&self, mut record: StoredSwap) -> Result<()> {
        record.validate()?;
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let index: Option<u32> = tx
            .query_row(
                "SELECT key_index FROM sdk_swap_keys WHERE wallet_id = ?1 AND swap_id = ?2",
                params![self.wallet_id, record.response.id()],
                |row| row.get(0),
            )
            .optional()?;
        if index.is_some_and(|index| index != record.key_index) {
            return Err(Error::Storage(
                "cannot replace a stored swap's key index".into(),
            ));
        }
        if let Some(mut old) = Self::read_record(&tx, &self.wallet_id, record.response.id())? {
            old.merge(record)?;
            record = old;
        }
        tx.execute("INSERT OR IGNORE INTO sdk_swap_keys(wallet_id, swap_id, key_index) VALUES (?1, ?2, ?3)",
            params![self.wallet_id, record.response.id(), record.key_index])?;
        tx.execute(
            "UPDATE sdk_key_indices SET next_index = MAX(next_index, ?1) WHERE wallet_id = ?2",
            params![record.key_index + 1, self.wallet_id],
        )?;
        Self::write_record(&tx, &self.wallet_id, &record)?;
        tx.commit()?;
        Ok(())
    }

    fn get_swap(&self, swap_id: &str) -> Result<Option<StoredSwap>> {
        let connection = self.connection()?;
        Self::read_record(&connection, &self.wallet_id, swap_id)
    }

    fn list_swaps(&self) -> Result<Vec<StoredSwap>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare("SELECT swap_id, record_json FROM sdk_swap_records WHERE wallet_id = ?1 ORDER BY swap_id")?;
        let rows = statement.query_map([&self.wallet_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (id, json) = row?;
            let record: StoredSwap =
                serde_json::from_str(&json).map_err(|e| Error::Storage(e.to_string()))?;
            record.validate()?;
            if record.response.id() != id {
                return Err(Error::Storage(
                    "swap record ID does not match its database key".into(),
                ));
            }
            Ok(record)
        })
        .collect()
    }

    fn update_metadata(&self, swap_id: &str, metadata: SwapMetadata) -> Result<()> {
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut record = Self::read_record(&tx, &self.wallet_id, swap_id)?
            .ok_or_else(|| Error::Storage("swap record not found".into()))?;
        record.metadata.merge(metadata);
        record.updated_at = super::record::now()?.max(record.updated_at);
        Self::write_record(&tx, &self.wallet_id, &record)?;
        tx.commit()?;
        Ok(())
    }

    fn delete_swap(&self, swap_id: &str) -> Result<()> {
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM sdk_swap_records WHERE wallet_id = ?1 AND swap_id = ?2",
            params![self.wallet_id, swap_id],
        )?;
        tx.execute(
            "DELETE FROM sdk_swap_keys WHERE wallet_id = ?1 AND swap_id = ?2",
            params![self.wallet_id, swap_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM sdk_swap_records WHERE wallet_id = ?1",
            [&self.wallet_id],
        )?;
        tx.execute(
            "DELETE FROM sdk_swap_keys WHERE wallet_id = ?1",
            [&self.wallet_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn ensure_next_key_index(&self, next_index: u32) -> Result<()> {
        if next_index > KEY_INDEX_LIMIT {
            return Err(Error::Storage("next key index exceeds 2^31".into()));
        }
        self.connection()?.execute(
            "UPDATE sdk_key_indices SET next_index = MAX(next_index, ?1) WHERE wallet_id = ?2",
            params![next_index, self.wallet_id],
        )?;
        Ok(())
    }

    fn next_key_index(&self) -> Result<u32> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let index: u32 = transaction.query_row(
            "SELECT next_index FROM sdk_key_indices WHERE wallet_id = ?1",
            [&self.wallet_id],
            |row| row.get(0),
        )?;
        if index >= KEY_INDEX_LIMIT {
            return Err(Error::Storage("key indices exhausted".into()));
        }
        transaction.execute(
            "UPDATE sdk_key_indices SET next_index = ?1 WHERE wallet_id = ?2",
            params![index + 1, self.wallet_id],
        )?;
        transaction.commit()?;
        Ok(index)
    }

    fn put_swap_key_index(&self, swap_id: &str, key_index: u32) -> Result<()> {
        if key_index >= KEY_INDEX_LIMIT {
            return Err(Error::Storage("key index must be below 2^31".into()));
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if Self::read_record(&transaction, &self.wallet_id, swap_id)?
            .is_some_and(|r| r.key_index != key_index)
        {
            return Err(Error::Storage(
                "cannot replace a stored swap's key index".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO sdk_swap_keys(wallet_id, swap_id, key_index) VALUES (?1, ?2, ?3)
             ON CONFLICT(wallet_id, swap_id) DO UPDATE SET key_index = excluded.key_index",
            params![self.wallet_id, swap_id, key_index],
        )?;
        // Imported indices must never be allocated again, even after a restart.
        transaction.execute(
            "UPDATE sdk_key_indices SET next_index = MAX(next_index, ?1) WHERE wallet_id = ?2",
            params![key_index + 1, self.wallet_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn get_swap_key_index(&self, swap_id: &str) -> Result<Option<u32>> {
        Ok(self
            .connection()?
            .query_row(
                "SELECT key_index FROM sdk_swap_keys WHERE wallet_id = ?1 AND swap_id = ?2",
                params![self.wallet_id, swap_id],
                |row| row.get(0),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_preserves_swap_keys_and_burned_indices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("swaps.db");
        {
            let storage = SqliteSwapStorage::open(&path, "wallet").unwrap();
            let index = storage.next_key_index().unwrap();
            storage.put_swap_key_index("swap", index).unwrap();
            // A failed request still consumes its index.
            assert_eq!(storage.next_key_index().unwrap(), 1);
        }
        let storage = SqliteSwapStorage::open(&path, "wallet").unwrap();
        assert_eq!(storage.get_swap_key_index("swap").unwrap(), Some(0));
        assert_eq!(storage.next_key_index().unwrap(), 2);
        assert_eq!(storage.get_swap_key_index("missing").unwrap(), None);
    }

    #[test]
    fn wallets_in_the_same_database_are_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("swaps.db");
        let first = SqliteSwapStorage::open(&path, "first").unwrap();
        let second = SqliteSwapStorage::open(&path, "second").unwrap();
        first.put_swap_key_index("swap", 7).unwrap();
        assert_eq!(second.get_swap_key_index("swap").unwrap(), None);
        assert_eq!(second.next_key_index().unwrap(), 0);
        assert_eq!(first.next_key_index().unwrap(), 8);
    }

    #[test]
    fn independent_connections_allocate_unique_indices_concurrently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("swaps.db");
        let stores: Vec<_> = (0..4)
            .map(|_| SqliteSwapStorage::open(&path, "wallet").unwrap())
            .collect();
        let handles: Vec<_> = stores
            .into_iter()
            .map(|store| {
                std::thread::spawn(move || {
                    (0..25)
                        .map(|_| store.next_key_index().unwrap())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut indices: Vec<_> = handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect();
        indices.sort_unstable();
        assert_eq!(indices, (0..100).collect::<Vec<_>>());
    }

    #[test]
    fn exhausted_indices_never_wrap_or_resume_after_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("swaps.db");
        {
            let storage = SqliteSwapStorage::open(&path, "wallet").unwrap();
            storage
                .put_swap_key_index("last", KEY_INDEX_LIMIT - 2)
                .unwrap();
            assert_eq!(storage.next_key_index().unwrap(), KEY_INDEX_LIMIT - 1);
            assert!(matches!(storage.next_key_index(), Err(Error::Storage(_))));
        }
        let storage = SqliteSwapStorage::open(&path, "wallet").unwrap();
        assert!(matches!(storage.next_key_index(), Err(Error::Storage(_))));
        assert!(
            storage
                .put_swap_key_index("invalid", KEY_INDEX_LIMIT)
                .is_err()
        );
    }

    #[test]
    fn invalid_path_returns_storage_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            SqliteSwapStorage::open(dir.path().join("missing/swaps.db"), "wallet"),
            Err(Error::Storage(_))
        ));
    }
}
