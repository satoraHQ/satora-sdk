use lendaswap_sdk::Client;
use lendaswap_sdk::EvmKeySource;
use lendaswap_sdk::InMemorySwapStorage;
use lendaswap_sdk::Signer;
use lendaswap_sdk::SqliteSwapStorage;
use lendaswap_sdk::StoredSwap;
use lendaswap_sdk::SwapMetadata;
use lendaswap_sdk::SwapStorage;
use lendaswap_sdk::types::GetSwapResponse;
use lendaswap_sdk::types::SwapStatus;
use serde_json::Value;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn record(index: u32) -> StoredSwap {
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/swap-responses.json")).unwrap();
    StoredSwap::new(
        index,
        serde_json::from_value(fixtures["evm_to_arkade"].clone()).unwrap(),
        SwapMetadata::default(),
    )
    .unwrap()
}

fn lifecycle(store: &dyn SwapStorage) {
    let mut original = record(7);
    original.metadata.bridge_recipient = Some("recipient".into());
    let id = original.response.id().to_owned();
    store.store_swap(original.clone()).unwrap();
    let mut refreshed = record(7);
    if let GetSwapResponse::EvmToArkade(r) = &mut refreshed.response {
        r.status = SwapStatus::ClientFunded;
    }
    store.store_swap(refreshed).unwrap();
    store
        .update_metadata(
            &id,
            SwapMetadata {
                evm_fund_txid: Some("tx".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let result = store.get_swap(&id).unwrap().unwrap();
    assert_eq!(result.stored_at, original.stored_at);
    assert_eq!(
        result.metadata.bridge_recipient.as_deref(),
        Some("recipient")
    );
    assert_eq!(result.metadata.evm_fund_txid.as_deref(), Some("tx"));
    assert_eq!(result.response.status(), &SwapStatus::ClientFunded);
    assert!(store.store_swap(record(8)).is_err());
    assert!(store.put_swap_key_index(&id, 8).is_err());
    assert_eq!(store.list_swaps().unwrap(), vec![result]);
    store.delete_swap(&id).unwrap();
    assert!(store.get_swap(&id).unwrap().is_none());
    assert!(store.get_swap_key_index(&id).unwrap().is_none());
    assert_eq!(store.next_key_index().unwrap(), 8);
    store.store_swap(record(12)).unwrap();
    store.clear().unwrap();
    assert!(store.list_swaps().unwrap().is_empty());
    assert!(store.get_swap_key_index(&id).unwrap().is_none());
    assert_eq!(store.next_key_index().unwrap(), 13);
    assert!(
        store
            .update_metadata("missing", SwapMetadata::default())
            .is_err()
    );
}

#[test]
fn memory_record_lifecycle_preserves_metadata_and_counter() {
    lifecycle(&InMemorySwapStorage::new());
}

#[test]
fn sqlite_record_lifecycle_preserves_metadata_and_counter() {
    let dir = tempfile::tempdir().unwrap();
    lifecycle(&SqliteSwapStorage::open(dir.path().join("swaps.db"), "wallet").unwrap());
}

#[test]
fn old_database_upgrades_without_losing_indices_and_reopens_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("swaps.db");
    let old = rusqlite::Connection::open(&path).unwrap();
    old.execute_batch("CREATE TABLE sdk_key_indices(wallet_id TEXT PRIMARY KEY, next_index INTEGER);
        CREATE TABLE sdk_swap_keys(wallet_id TEXT, swap_id TEXT, key_index INTEGER, PRIMARY KEY(wallet_id, swap_id));
        INSERT INTO sdk_key_indices VALUES ('wallet', 55);").unwrap();
    let record = record(7);
    old.execute(
        "INSERT INTO sdk_swap_keys VALUES ('wallet', ?1, 7)",
        [record.response.id()],
    )
    .unwrap();
    drop(old);
    {
        let store = SqliteSwapStorage::open(&path, "wallet").unwrap();
        assert_eq!(
            store.get_swap_key_index(record.response.id()).unwrap(),
            Some(7)
        );
        assert!(store.get_swap(record.response.id()).unwrap().is_none());
        store.store_swap(record.clone()).unwrap();
    }
    let reopened = SqliteSwapStorage::open(&path, "wallet").unwrap();
    assert_eq!(
        reopened.get_swap(record.response.id()).unwrap(),
        Some(record)
    );
    assert_eq!(reopened.next_key_index().unwrap(), 55);
    let other = SqliteSwapStorage::open(&path, "other").unwrap();
    other.clear().unwrap();
    assert_eq!(reopened.list_swaps().unwrap().len(), 1);
}

#[test]
fn concurrent_connections_merge_disjoint_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("swaps.db");
    let store = SqliteSwapStorage::open(&path, "wallet").unwrap();
    let record = record(7);
    store.store_swap(record.clone()).unwrap();
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let store = SqliteSwapStorage::open(&path, "wallet").unwrap();
            let id = record.response.id().to_owned();
            std::thread::spawn(move || {
                let metadata = if i == 0 {
                    SwapMetadata {
                        bridge_recipient: Some("recipient".into()),
                        ..Default::default()
                    }
                } else {
                    SwapMetadata {
                        evm_fund_txid: Some("tx".into()),
                        ..Default::default()
                    }
                };
                store.update_metadata(&id, metadata).unwrap();
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let metadata = store
        .get_swap(record.response.id())
        .unwrap()
        .unwrap()
        .metadata;
    assert_eq!(metadata.bridge_recipient.as_deref(), Some("recipient"));
    assert_eq!(metadata.evm_fund_txid.as_deref(), Some("tx"));
}

#[test]
fn bip44_key_matches_known_ethereum_wallet_vector() {
    let signer = Signer::from_mnemonic(MNEMONIC).unwrap();
    assert_eq!(
        signer.derive_wallet_evm_key().unwrap().address,
        "0x9858effd232b4033e47d90003d41ec34ecaeda94"
    );
    assert_ne!(
        signer.derive_evm_key(0).unwrap().address,
        signer.derive_legacy_evm_key(0).unwrap().address
    );
}

#[tokio::test]
async fn recovery_identifies_rust_typescript_and_external_owners() {
    let signer = Signer::from_mnemonic(MNEMONIC).unwrap();
    let external = "0x1111111111111111111111111111111111111111".to_owned();
    for (address, expected) in [
        (
            signer.derive_evm_key(0).unwrap().address,
            EvmKeySource::RustPerSwap,
        ),
        (
            signer.derive_wallet_evm_key().unwrap().address,
            EvmKeySource::Bip44Wallet,
        ),
        (
            signer.derive_legacy_evm_key(0).unwrap().address,
            EvmKeySource::LegacySwapKey,
        ),
        (
            external.clone(),
            EvmKeySource::External { address: external },
        ),
    ] {
        let server = MockServer::start().await;
        let mut response = serde_json::to_value(record(0).response).unwrap();
        response["client_evm_address"] = json!(address);
        response["hash_lock"] = json!(hex::encode(signer.derive_swap_params(0).unwrap().hash_lock));
        let id = response["id"].as_str().unwrap().to_owned();
        response["index"] = json!(0);
        Mock::given(method("POST")).and(path("/swap/recover"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"swaps":[response],"highest_index":0,"next_index":1,"scanned_until":20,"complete":true})))
            .mount(&server).await;
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .build()
            .unwrap();
        client.recover_all_swaps(None).await.unwrap();
        assert_eq!(
            client
                .get_stored_swap(&id)
                .unwrap()
                .unwrap()
                .metadata
                .evm_key_source,
            Some(expected)
        );
    }
}

#[tokio::test]
async fn interrupted_recovery_keeps_completed_pages_after_restart() {
    use wiremock::matchers::body_partial_json;
    let server = MockServer::start().await;
    let signer = Signer::from_mnemonic(MNEMONIC).unwrap();
    let mut response = serde_json::to_value(record(3).response).unwrap();
    response["hash_lock"] = json!(hex::encode(signer.derive_swap_params(3).unwrap().hash_lock));
    response["index"] = json!(3);
    let id = response["id"].as_str().unwrap().to_owned();
    Mock::given(path("/swap/recover")).and(body_partial_json(json!({"start_index":0})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"swaps":[response],"highest_index":3,"next_index":4,"scanned_until":10,"complete":false})))
        .mount(&server).await;
    Mock::given(path("/swap/recover"))
        .and(body_partial_json(json!({"start_index":11})))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("swaps.db");
    {
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage_path(&db)
            .build()
            .unwrap();
        assert!(client.recover_all_swaps(None).await.is_err());
    }
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage_path(&db)
        .build()
        .unwrap();
    assert_eq!(client.get_stored_swap(&id).unwrap().unwrap().key_index, 3);
}
