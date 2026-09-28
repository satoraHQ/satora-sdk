use bitcoin::Network;
use bitcoin::bip32::Xpriv;
use lendaswap_sdk::Client;
use lendaswap_sdk::Error;
use lendaswap_sdk::InMemoryWalletStorage;
use lendaswap_sdk::Signer;
use lendaswap_sdk::SqliteWalletStorage;
use lendaswap_sdk::WalletStorage;
use lendaswap_sdk::types::GetSwapResponse;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const OTHER: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";

fn stored(wallet: &Path) -> Option<String> {
    SqliteWalletStorage::open(wallet)
        .unwrap()
        .get_mnemonic()
        .unwrap()
}

fn write_raw_mnemonic(wallet: &Path, mnemonic: &str) {
    SqliteWalletStorage::open(wallet).unwrap();
    rusqlite::Connection::open(wallet)
        .unwrap()
        .execute("UPDATE wallet SET mnemonic = ?1 WHERE id = 1", [mnemonic])
        .unwrap();
}

#[tokio::test]
async fn generated_seed_persists_and_reopens_swaps() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let wallet = dir.path().join("wallet.db");
    let db = dir.path().join("swaps.db");
    let build = || {
        Client::builder()
            .base_url(server.uri())
            .wallet_storage_path(&wallet)
            .storage_path(&db)
            .build()
    };
    let client = build().unwrap();
    let mnemonic = client.mnemonic().unwrap().to_owned();
    assert_eq!(mnemonic.split(' ').count(), 12);
    assert_eq!(stored(&wallet).as_ref(), Some(&mnemonic));

    let signer = Signer::from_mnemonic(&mnemonic).unwrap();
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/swap-responses.json")).unwrap();
    let swap: GetSwapResponse = serde_json::from_value(fixtures["evm_to_arkade"].clone()).unwrap();
    let mut response = serde_json::to_value(swap).unwrap();
    response["hash_lock"] = json!(hex::encode(signer.derive_swap_params(3).unwrap().hash_lock));
    response["index"] = json!(3);
    let id = response["id"].as_str().unwrap().to_owned();
    Mock::given(method("POST")).and(path("/swap/recover"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"swaps":[response],"highest_index":3,"next_index":4,"scanned_until":10,"complete":true})))
        .mount(&server).await;
    client.recover_all_swaps(None).await.unwrap();
    drop(client);

    let client = build().unwrap();
    assert_eq!(client.mnemonic(), Some(mnemonic.as_str()));
    assert_eq!(client.get_stored_swap(&id).unwrap().unwrap().key_index, 3);
}

#[test]
fn supplied_mnemonic_persists_when_empty() {
    let storage = InMemoryWalletStorage::shared();
    let client = Client::builder()
        .mnemonic(format!("  {}\n", MNEMONIC.to_uppercase()))
        .wallet_storage(storage.clone())
        .build()
        .unwrap();
    assert_eq!(client.mnemonic(), Some(MNEMONIC));
    assert_eq!(storage.get_mnemonic().unwrap().as_deref(), Some(MNEMONIC));
    let client = Client::builder()
        .wallet_storage(storage.clone())
        .build()
        .unwrap();
    assert_eq!(client.mnemonic(), Some(MNEMONIC));
    Client::builder()
        .mnemonic(MNEMONIC)
        .wallet_storage(storage)
        .build()
        .unwrap();
}

#[test]
fn supplied_mnemonic_matches_differently_spelled_stored_copy() {
    let dir = tempfile::tempdir().unwrap();
    let wallet = dir.path().join("wallet.db");
    let spelled = format!(" {} ", MNEMONIC.to_uppercase());
    write_raw_mnemonic(&wallet, &spelled);
    let client = Client::builder()
        .mnemonic(MNEMONIC)
        .wallet_storage_path(&wallet)
        .build()
        .unwrap();
    assert_eq!(client.mnemonic(), Some(MNEMONIC));
    assert_eq!(stored(&wallet), Some(spelled));
}

#[test]
fn mismatched_mnemonic_rejected_and_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let wallet = dir.path().join("wallet.db");
    SqliteWalletStorage::open(&wallet)
        .unwrap()
        .set_mnemonic(MNEMONIC)
        .unwrap();
    let err = Client::builder()
        .mnemonic(OTHER)
        .wallet_storage_path(&wallet)
        .build()
        .unwrap_err();
    assert!(
        matches!(&err, Error::InvalidSigner(m) if m.contains("different mnemonic")),
        "{err}"
    );
    assert_eq!(stored(&wallet).as_deref(), Some(MNEMONIC));
}

#[test]
fn invalid_stored_mnemonic_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let wallet = dir.path().join("wallet.db");
    write_raw_mnemonic(&wallet, "not a mnemonic");
    for supplied in [None, Some(MNEMONIC)] {
        let mut builder = Client::builder().wallet_storage_path(&wallet);
        if let Some(mnemonic) = supplied {
            builder = builder.mnemonic(mnemonic);
        }
        let err = builder.build().unwrap_err();
        assert!(
            matches!(&err, Error::InvalidSigner(m) if m.contains("invalid mnemonic")),
            "{err}"
        );
        assert_eq!(stored(&wallet).as_deref(), Some("not a mnemonic"));
    }
}

/// Reports no stored mnemonic, then answers `get_or_init_mnemonic` with
/// `outcome`: `Ok` as when another process saved first, `Err` as a failed save.
struct RacedStorage {
    outcome: Result<&'static str, &'static str>,
    candidates: Mutex<Vec<String>>,
}

impl RacedStorage {
    fn shared(outcome: Result<&'static str, &'static str>) -> Arc<Self> {
        Arc::new(Self {
            outcome,
            candidates: Mutex::default(),
        })
    }

    fn candidates(&self) -> Vec<String> {
        self.candidates.lock().unwrap().clone()
    }
}

impl WalletStorage for RacedStorage {
    fn get_mnemonic(&self) -> lendaswap_sdk::Result<Option<String>> {
        Ok(None)
    }

    fn set_mnemonic(&self, _: &str) -> lendaswap_sdk::Result<()> {
        Err(Error::Storage("unexpected set_mnemonic".into()))
    }

    fn get_or_init_mnemonic(&self, candidate: &str) -> lendaswap_sdk::Result<String> {
        self.candidates.lock().unwrap().push(candidate.to_owned());
        self.outcome
            .map(str::to_owned)
            .map_err(|message| Error::Storage(message.into()))
    }

    fn clear(&self) -> lendaswap_sdk::Result<()> {
        Err(Error::Storage("unexpected clear".into()))
    }
}

#[test]
fn generated_seed_yields_to_the_stored_one() {
    let storage = RacedStorage::shared(Ok(OTHER));
    let client = Client::builder()
        .wallet_storage(storage.clone())
        .build()
        .unwrap();
    assert_eq!(client.mnemonic(), Some(OTHER));
    let candidates = storage.candidates();
    assert_eq!(candidates.len(), 1);
    assert_ne!(candidates[0], OTHER);
}

#[test]
fn failed_seed_save_fails_build() {
    for supplied in [None, Some(MNEMONIC)] {
        let storage = RacedStorage::shared(Err("disk full"));
        let mut builder = Client::builder().wallet_storage(storage.clone());
        if let Some(mnemonic) = supplied {
            builder = builder.mnemonic(mnemonic);
        }
        let err = builder.build().unwrap_err();
        assert!(
            matches!(&err, Error::Storage(m) if m == "disk full"),
            "{err}"
        );
        assert_eq!(storage.candidates().len(), 1);
    }
}

#[test]
fn xprv_with_wallet_storage_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let wallet = dir.path().join("wallet.db");
    let seed = bip39::Mnemonic::from_str(MNEMONIC).unwrap().to_seed("");
    let xprv = Xpriv::new_master(Network::Bitcoin, &seed)
        .unwrap()
        .to_string();
    let err = Client::builder()
        .xprv(&xprv)
        .wallet_storage_path(&wallet)
        .build()
        .unwrap_err();
    assert!(matches!(err, Error::InvalidSigner(_)), "{err}");
    assert!(!wallet.exists());

    let storage = InMemoryWalletStorage::shared();
    let err = Client::builder()
        .xprv(xprv)
        .wallet_storage(storage.clone())
        .build()
        .unwrap_err();
    assert!(matches!(err, Error::InvalidSigner(_)), "{err}");
    assert_eq!(storage.get_mnemonic().unwrap(), None);
}

#[test]
fn same_file_rejected() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let db = dir.path().join("swaps.db");
    // `Path` equality already ignores `.`; only canonicalizing resolves `..`.
    let spellings = [
        dir.path().join(".").join("swaps.db"),
        dir.path().join("sub").join("..").join("swaps.db"),
    ];
    let same_file = |wallet: &Path| {
        let err = Client::builder()
            .wallet_storage_path(wallet)
            .storage_path(&db)
            .build()
            .unwrap_err();
        assert!(
            matches!(err, Error::Storage(_)),
            "{}: {err}",
            wallet.display()
        );
    };
    for wallet in &spellings {
        same_file(wallet);
    }
    assert!(!db.exists());

    Client::builder()
        .mnemonic(MNEMONIC)
        .storage_path(&db)
        .build()
        .unwrap();
    same_file(&db);
    for wallet in &spellings {
        same_file(wallet);
    }
}

#[test]
fn wallet_storage_and_path_are_exclusive() {
    let dir = tempfile::tempdir().unwrap();
    let err = Client::builder()
        .wallet_storage(InMemoryWalletStorage::shared())
        .wallet_storage_path(dir.path().join("wallet.db"))
        .build()
        .unwrap_err();
    assert!(matches!(err, Error::Storage(_)), "{err}");
}

#[test]
fn no_seed_no_storage_still_errors() {
    let err = Client::builder().build().unwrap_err();
    assert!(
        matches!(&err, Error::InvalidSigner(m) if m.contains("wallet_storage")),
        "{err}"
    );
}

#[test]
fn swap_database_never_holds_the_seed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("swaps.db");
    let wallet = dir.path().join("wallet.db");
    let tables = |file: &Path| -> Vec<String> {
        let connection = rusqlite::Connection::open(file).unwrap();
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let contains = |file: &Path, needle: &str| {
        std::fs::read(file)
            .unwrap()
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    };

    Client::builder()
        .mnemonic(MNEMONIC)
        .storage_path(&db)
        .build()
        .unwrap();
    assert!(!tables(&db).contains(&"wallet".to_owned()));
    assert!(!contains(&db, MNEMONIC));
    assert!(!wallet.exists());

    let client = Client::builder()
        .wallet_storage_path(&wallet)
        .storage_path(&db)
        .build()
        .unwrap();
    let generated = client.mnemonic().unwrap();
    assert!(!tables(&db).contains(&"wallet".to_owned()));
    assert!(!contains(&db, generated));
    assert!(contains(&wallet, generated));
}
