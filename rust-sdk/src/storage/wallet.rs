use crate::Error;
use crate::Result;
use crate::Signer;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::TransactionBehavior;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::Duration;

/// Persistence for the wallet seed, like the TypeScript `WalletStorage`.
///
/// Used by [`crate::ClientBuilder::wallet_storage`] to reuse, save or generate
/// the mnemonic. Key indices stay in [`super::SwapStorage`]. The store holds
/// the seed, so protect and back it up like the seed itself.
pub trait WalletStorage: Send + Sync {
    /// The stored mnemonic, or `None` when none is stored.
    fn get_mnemonic(&self) -> Result<Option<String>>;

    /// Store `mnemonic`, replacing any stored one. Swaps made with a replaced
    /// mnemonic can only be claimed or refunded with a backup of it.
    fn set_mnemonic(&self, mnemonic: &str) -> Result<()>;

    /// Store `candidate` only if no mnemonic is stored, then return the stored
    /// one. Must be atomic: concurrent callers with different candidates must
    /// all get the same result.
    fn get_or_init_mnemonic(&self, candidate: &str) -> Result<String>;

    /// Remove the stored mnemonic.
    fn clear(&self) -> Result<()>;
}

fn normalized(mnemonic: &str) -> Result<String> {
    Signer::from_mnemonic(mnemonic)?
        .mnemonic()
        .map(str::to_owned)
        .ok_or_else(|| Error::InvalidSigner("mnemonic signer has no mnemonic".into()))
}

/// Seed store that lives for the life of the process. Useful for tests.
#[derive(Default)]
pub struct InMemoryWalletStorage {
    mnemonic: Mutex<Option<String>>,
}

impl InMemoryWalletStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Wrap in `Arc` for use with [`crate::ClientBuilder::wallet_storage`].
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> Result<MutexGuard<'_, Option<String>>> {
        self.mnemonic
            .lock()
            .map_err(|_| Error::Storage("wallet storage lock poisoned".into()))
    }
}

impl std::fmt::Debug for InMemoryWalletStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryWalletStorage")
            .field("mnemonic", &"<redacted>")
            .finish()
    }
}

impl WalletStorage for InMemoryWalletStorage {
    fn get_mnemonic(&self) -> Result<Option<String>> {
        Ok(self.lock()?.clone())
    }

    fn set_mnemonic(&self, mnemonic: &str) -> Result<()> {
        let mnemonic = normalized(mnemonic)?;
        *self.lock()? = Some(mnemonic);
        Ok(())
    }

    fn get_or_init_mnemonic(&self, candidate: &str) -> Result<String> {
        let candidate = normalized(candidate)?;
        Ok(self.lock()?.get_or_insert(candidate).clone())
    }

    fn clear(&self) -> Result<()> {
        *self.lock()? = None;
        Ok(())
    }
}

/// Seed store in a SQLite file, in plaintext.
///
/// Uses the TypeScript `SqliteWalletStorage` table, so a wallet file written
/// by the TypeScript SDK opens directly. Its `key_index` column is left alone.
/// Keep this file separate from the swap database.
pub struct SqliteWalletStorage {
    connection: Mutex<Connection>,
}

impl SqliteWalletStorage {
    /// Open or create a wallet file in an existing directory. On Unix a new
    /// file is created readable by its owner only. Names SQLite treats
    /// specially (empty, `:memory:`, a `file:` URI) are refused; use
    /// [`InMemoryWalletStorage`] for a store that is not saved.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        // For these SQLite would write the seed to a file other than the one
        // created owner-only below (the bundled build parses `file:` URIs
        // whatever the open flags), or to memory or a temporary file.
        let name = path.as_os_str().as_encoded_bytes();
        if name.is_empty() || name == b":memory:" || name.starts_with(b"file:") {
            return Err(Error::Storage(format!(
                "wallet file {} is not a plain file path",
                path.display()
            )));
        }
        #[cfg(unix)]
        create_owner_only(path)?;
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS wallet (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                mnemonic TEXT,
                key_index INTEGER NOT NULL DEFAULT 0
            );
            INSERT OR IGNORE INTO wallet (id, key_index) VALUES (1, 0);",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| Error::Storage("wallet storage lock poisoned".into()))
    }

    fn read(connection: &Connection) -> Result<Option<String>> {
        let mnemonic: Option<Option<String>> = connection
            .query_row("SELECT mnemonic FROM wallet WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()?;
        // The TypeScript builder treats an empty value as no mnemonic.
        Ok(mnemonic.flatten().filter(|m| !m.is_empty()))
    }
}

#[cfg(unix)]
fn create_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(Error::Storage(format!(
            "cannot create wallet file {}: {error}",
            path.display()
        ))),
    }
}

impl std::fmt::Debug for SqliteWalletStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteWalletStorage")
            .finish_non_exhaustive()
    }
}

impl WalletStorage for SqliteWalletStorage {
    fn get_mnemonic(&self) -> Result<Option<String>> {
        Self::read(&*self.connection()?)
    }

    fn set_mnemonic(&self, mnemonic: &str) -> Result<()> {
        let mnemonic = normalized(mnemonic)?;
        self.connection()?.execute(
            "INSERT INTO wallet (id, mnemonic) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET mnemonic = excluded.mnemonic",
            [mnemonic],
        )?;
        Ok(())
    }

    fn get_or_init_mnemonic(&self, candidate: &str) -> Result<String> {
        let candidate = normalized(candidate)?;
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO wallet (id, mnemonic) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET mnemonic = excluded.mnemonic
             WHERE coalesce(wallet.mnemonic, '') = ''",
            [candidate],
        )?;
        let stored = Self::read(&tx)?
            .ok_or_else(|| Error::Storage("wallet row has no mnemonic after saving".into()))?;
        tx.commit()?;
        Ok(stored)
    }

    fn clear(&self) -> Result<()> {
        self.connection()?
            .execute("UPDATE wallet SET mnemonic = NULL WHERE id = 1", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const SECOND: &str =
        "legal winner thank year wave sausage worth useful legal winner thank yellow";

    fn stores(dir: &tempfile::TempDir) -> [Box<dyn WalletStorage>; 2] {
        [
            Box::new(InMemoryWalletStorage::new()),
            Box::new(SqliteWalletStorage::open(dir.path().join("wallet.db")).unwrap()),
        ]
    }

    #[test]
    fn get_or_init_keeps_first() {
        let dir = tempfile::tempdir().unwrap();
        for store in stores(&dir) {
            assert_eq!(store.get_mnemonic().unwrap(), None);
            assert_eq!(store.get_or_init_mnemonic(FIRST).unwrap(), FIRST);
            assert_eq!(store.get_or_init_mnemonic(SECOND).unwrap(), FIRST);
            assert_eq!(store.get_mnemonic().unwrap().as_deref(), Some(FIRST));
        }
    }

    #[test]
    fn writes_are_validated_and_normalized() {
        let dir = tempfile::tempdir().unwrap();
        for store in stores(&dir) {
            assert!(matches!(
                store.set_mnemonic("abandon abandon"),
                Err(Error::InvalidSigner(_))
            ));
            assert!(store.get_or_init_mnemonic("not a mnemonic").is_err());
            assert_eq!(store.get_mnemonic().unwrap(), None);
            store
                .set_mnemonic(&format!(" {}\n", SECOND.to_uppercase().replace(' ', "  ")))
                .unwrap();
            assert_eq!(store.get_mnemonic().unwrap().as_deref(), Some(SECOND));
            store.set_mnemonic(FIRST).unwrap();
            assert_eq!(store.get_mnemonic().unwrap().as_deref(), Some(FIRST));
        }
    }

    #[test]
    fn clear_removes_only_mnemonic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wallet.db");
        let store = SqliteWalletStorage::open(&path).unwrap();
        store.set_mnemonic(FIRST).unwrap();
        let raw = Connection::open(&path).unwrap();
        raw.execute("UPDATE wallet SET key_index = 5 WHERE id = 1", [])
            .unwrap();
        store.clear().unwrap();
        assert_eq!(store.get_mnemonic().unwrap(), None);
        let key_index: u32 = raw
            .query_row("SELECT key_index FROM wallet WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(key_index, 5);

        let memory = InMemoryWalletStorage::new();
        memory.set_mnemonic(FIRST).unwrap();
        memory.clear().unwrap();
        assert_eq!(memory.get_mnemonic().unwrap(), None);
    }

    #[test]
    fn sqlite_reopen_returns_mnemonic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wallet.db");
        SqliteWalletStorage::open(&path)
            .unwrap()
            .get_or_init_mnemonic(FIRST)
            .unwrap();
        let store = SqliteWalletStorage::open(&path).unwrap();
        assert_eq!(store.get_mnemonic().unwrap().as_deref(), Some(FIRST));
    }

    #[test]
    fn sqlite_concurrent_init_agrees() {
        const THREADS: usize = 4;
        let dir = tempfile::tempdir().unwrap();
        let candidates: Vec<String> = (0..THREADS)
            .map(|_| Signer::generate(12).unwrap().mnemonic().unwrap().to_owned())
            .collect();
        // One round often passes even against a plain read-then-write
        // get_or_init, so repeat on fresh files.
        for round in 0..20 {
            let path = dir.path().join(format!("wallet-{round}.db"));
            let stores: Vec<_> = (0..THREADS)
                .map(|_| SqliteWalletStorage::open(&path).unwrap())
                .collect();
            let barrier = &std::sync::Barrier::new(THREADS);
            let results: Vec<String> = std::thread::scope(|scope| {
                let handles: Vec<_> = stores
                    .iter()
                    .zip(&candidates)
                    .map(|(store, candidate)| {
                        scope.spawn(move || {
                            barrier.wait();
                            store.get_or_init_mnemonic(candidate).unwrap()
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            assert!(
                results.iter().all(|r| *r == results[0]),
                "round {round}: {results:?}"
            );
            let stored = SqliteWalletStorage::open(&path)
                .unwrap()
                .get_mnemonic()
                .unwrap();
            assert_eq!(stored.as_ref(), Some(&results[0]), "round {round}");
        }
    }

    #[test]
    fn sqlite_special_names_refused() {
        let dir = tempfile::tempdir().unwrap();
        let wallet = dir.path().join("wallet.db");
        for name in [
            String::new(),
            ":memory:".to_owned(),
            "file::memory:".to_owned(),
            format!("file:{}", wallet.display()),
            format!("file://{}?mode=rwc", wallet.display()),
        ] {
            let result = SqliteWalletStorage::open(&name);
            assert!(
                matches!(&result, Err(Error::Storage(m)) if m.contains("not a plain file path")),
                "{name}: {result:?}"
            );
        }
        assert!(!wallet.exists());
    }

    #[test]
    fn sqlite_opens_typescript_wallet_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lendaswap.db");
        let raw = Connection::open(&path).unwrap();
        raw.execute_batch(
            "CREATE TABLE IF NOT EXISTS wallet (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                mnemonic TEXT,
                key_index INTEGER NOT NULL DEFAULT 0
            );
            INSERT OR IGNORE INTO wallet (id, key_index) VALUES (1, 0);
            UPDATE wallet SET mnemonic = '', key_index = 7 WHERE id = 1;",
        )
        .unwrap();
        let store = SqliteWalletStorage::open(&path).unwrap();
        assert_eq!(store.get_mnemonic().unwrap(), None);
        assert_eq!(store.get_or_init_mnemonic(SECOND).unwrap(), SECOND);

        raw.execute("UPDATE wallet SET mnemonic = ?1 WHERE id = 1", [FIRST])
            .unwrap();
        assert_eq!(store.get_mnemonic().unwrap().as_deref(), Some(FIRST));
        assert_eq!(store.get_or_init_mnemonic(SECOND).unwrap(), FIRST);
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_wallet_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wallet.db");
        SqliteWalletStorage::open(&path)
            .unwrap()
            .set_mnemonic(FIRST)
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "mode {mode:o}");
    }

    #[test]
    fn debug_redacts_mnemonic() {
        let store = InMemoryWalletStorage::new();
        store.set_mnemonic(FIRST).unwrap();
        assert!(!format!("{store:?}").contains("abandon"));
    }
}
