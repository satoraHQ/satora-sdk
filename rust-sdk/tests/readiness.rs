//! Current API compatibility and restart regressions, without external services.
use lendaswap_sdk::Client;
use lendaswap_sdk::Error;
use lendaswap_sdk::InMemorySwapStorage;
use lendaswap_sdk::LogLevel;
use lendaswap_sdk::LogRecord;
use lendaswap_sdk::Signer;
use lendaswap_sdk::Swap;
use lendaswap_sdk::SwapFunding;
use lendaswap_sdk::SwapStorage;
use lendaswap_sdk::types::Address;
use lendaswap_sdk::types::GetSwapResponse;
use lendaswap_sdk::types::LightningDestination;
use lendaswap_sdk::types::QuoteAmount;
use lendaswap_sdk::types::SwapDirection;
use lendaswap_sdk::types::SwapStatus;
use lendaswap_sdk::types::TokenId;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_partial_json;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn fixture(direction: &str) -> Value {
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/swap-responses.json")).unwrap();
    fixtures[direction].clone()
}

/// A creation answer for `direction` that names the deposit address an
/// EVM-sourced creation at key index 0 sends, as the server does.
fn created(direction: &str) -> Value {
    let mut response = fixture(direction);
    if direction.starts_with("evm_") {
        response["client_evm_address"] = json!(
            Signer::from_mnemonic(MNEMONIC)
                .unwrap()
                .derive_evm_key(0)
                .unwrap()
                .address
        );
    }
    response
}

/// Real invoices and swap responses from the regtest stack.
fn lightning_vectors() -> Value {
    serde_json::from_str(include_str!("../../fixtures/lightning-invoices.json")).unwrap()
}

fn invoice(name: &str) -> String {
    lightning_vectors()["invoices"][name]["bolt11"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The shared vector paying `hash_lock`, a hex hash with or without `0x`.
fn invoice_paying(hash_lock: &str) -> String {
    let vectors = lightning_vectors();
    let swaps = vectors["swaps"].as_object().unwrap().values().map(|swap| {
        let response = &swap["response"];
        (
            &swap["invoice_payment_hash"],
            response
                .get("bolt11_invoice")
                .unwrap_or(&response["client_lightning_invoice"]),
        )
    });
    vectors["invoices"]
        .as_object()
        .unwrap()
        .values()
        .map(|invoice| (&invoice["payment_hash"], &invoice["bolt11"]))
        .chain(swaps)
        .find(|(hash, _)| *hash == hash_lock.trim_start_matches("0x"))
        .and_then(|(_, invoice)| invoice.as_str())
        .unwrap_or_else(|| panic!("no shared invoice pays {hash_lock}"))
        .to_owned()
}

/// A Lightning→Arkade or Lightning→EVM creation response whose hold invoice pays
/// the hash of the preimage at `key_index`, as creation requires.
fn hold_invoice_response(direction: &str, key_index: u32) -> Value {
    let params = Signer::from_mnemonic(MNEMONIC)
        .unwrap()
        .derive_swap_params(key_index)
        .unwrap();
    let hash_lock = format!("0x{}", hex::encode(params.hash_lock));
    let mut response = fixture(direction);
    response["bolt11_invoice"] = json!(invoice_paying(&hash_lock));
    response["hash_lock"] = json!(hash_lock);
    response
}

/// A Bitcoin→Arkade or Bitcoin→EVM creation response whose HTLC the client's keys
/// at `key_index` rebuild, as creation requires.
fn bitcoin_htlc_response(direction: &str, key_index: u32) -> Value {
    use bitcoin::hashes::Hash;
    use bitcoin::hashes::hash160;
    use bitcoin::secp256k1::PublicKey;
    let signer = Signer::from_mnemonic(MNEMONIC).unwrap();
    let params = signer.derive_swap_params(key_index).unwrap();
    let server =
        PublicKey::from_slice(&signer.derive_swap_params(999).unwrap().public_key).unwrap();
    let htlc = lendaswap_sdk::bitcoin::BitcoinHtlc {
        network: bitcoin::Network::Regtest,
        hash_lock: hash160::Hash::hash(&params.preimage).to_byte_array(),
        claimer: server.x_only_public_key().0,
        refunder: PublicKey::from_slice(&params.public_key)
            .unwrap()
            .x_only_public_key()
            .0,
        refund_locktime: 1,
        script_version: 1,
    };
    let (hash_lock, server_pk) = match direction {
        "btc_to_arkade" => ("hash_lock", "server_vhtlc_pk"),
        _ => ("btc_hash_lock", "btc_server_pk"),
    };
    let mut response = fixture(direction);
    response[hash_lock] = json!(hex::encode(htlc.hash_lock));
    response[server_pk] = json!(server.to_string());
    response["btc_htlc_address"] = json!(htlc.address().unwrap().to_string());
    response
}

fn client(server: &MockServer) -> Client {
    Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .build()
        .unwrap()
}

/// A client whose SDK events are collected at debug level and above.
fn recording_client(server: &MockServer) -> (Client, Arc<Mutex<Vec<LogRecord>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = records.clone();
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .logger(Arc::new(move |record: &LogRecord| {
            sink.lock().unwrap().push(record.clone())
        }))
        .log_level(LogLevel::Debug)
        .build()
        .unwrap();
    (client, records)
}

fn assert_schema(value: &Value, name: &str) {
    let spec: Value = serde_json::from_str(include_str!("../openapi.json")).unwrap();
    let mut schema = spec["components"]["schemas"][name].clone();
    schema["components"] = spec["components"].clone();
    // The server's TokenId oneOf overlaps: "btc" is also a string.
    // Keep the direction discriminator's oneOf strict.
    let token_id = schema["components"]["schemas"]["TokenId"]
        .as_object_mut()
        .unwrap();
    let variants = token_id.remove("oneOf").unwrap();
    token_id.insert("anyOf".into(), variants);
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<_> = validator
        .iter_errors(value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{name}: {errors:?}");
}

#[test]
fn every_swap_direction_deserializes_and_round_trips_the_current_schema() {
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/swap-responses.json")).unwrap();
    for (direction, value) in fixtures.as_object().unwrap() {
        let swap: GetSwapResponse = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(swap.status(), &SwapStatus::Pending, "{direction}");
        let encoded = serde_json::to_value(swap).unwrap();
        assert_schema(&encoded, "GetSwapResponse");
    }
}

#[test]
fn swap_projection_carries_direction_and_deadlines_for_every_direction() {
    const VHTLC: i64 = 1_800_000_001;
    const BTC: i64 = 1_800_000_002;
    const EVM: i64 = 1_800_000_003;
    const INVOICE: i64 = 1_800_000_004;
    // [vhtlc_refund_locktime, btc_refund_locktime, evm_refund_locktime, invoice_expires_at]
    let cases = [
        (
            "btc_to_arkade",
            SwapDirection::BtcToArkade,
            [Some(VHTLC), Some(BTC), None, None],
        ),
        (
            "bitcoin_to_evm",
            SwapDirection::BitcoinToEvm,
            [None, Some(BTC), Some(EVM), None],
        ),
        (
            "arkade_to_evm",
            SwapDirection::ArkadeToEvm,
            [Some(VHTLC), None, Some(EVM), None],
        ),
        (
            "lightning_to_evm",
            SwapDirection::LightningToEvm,
            [None, None, Some(EVM), Some(INVOICE)],
        ),
        (
            "evm_to_bitcoin",
            SwapDirection::EvmToBitcoin,
            [None, Some(BTC), Some(EVM), None],
        ),
        (
            "evm_to_lightning",
            SwapDirection::EvmToLightning,
            [None, None, Some(EVM), Some(INVOICE)],
        ),
        (
            "evm_to_arkade",
            SwapDirection::EvmToArkade,
            [Some(VHTLC), None, Some(EVM), None],
        ),
        (
            "lightning_to_arkade",
            SwapDirection::LightningToArkade,
            [Some(VHTLC), None, None, Some(INVOICE)],
        ),
        (
            "arkade_to_lightning",
            SwapDirection::ArkadeToLightning,
            [Some(VHTLC), None, None, Some(INVOICE)],
        ),
        (
            "bitcoin_to_lightning",
            SwapDirection::BitcoinToLightning,
            [None, Some(BTC), None, Some(INVOICE)],
        ),
    ];
    for (name, direction, deadlines) in cases {
        let mut value = fixture(name);
        value["created_at"] = json!("2026-10-02T12:00:00Z");
        // Decoding drops the fields a direction does not carry.
        value["vhtlc_refund_locktime"] = json!(VHTLC);
        value["btc_refund_locktime"] = json!(BTC);
        value["evm_refund_locktime"] = json!(EVM);
        value["invoice_expires_at"] = json!(INVOICE);
        let swap = Swap::from(serde_json::from_value::<GetSwapResponse>(value).unwrap());
        assert_eq!(swap.direction, direction, "{name}");
        assert_eq!(swap.created_at, "2026-10-02T12:00:00Z", "{name}");
        assert_eq!(
            [
                swap.vhtlc_refund_locktime,
                swap.btc_refund_locktime,
                swap.evm_refund_locktime,
                swap.invoice_expires_at,
            ],
            deadlines,
            "{name}"
        );
    }
}

#[test]
fn swap_projection_drops_out_of_range_locktimes_and_unset_invoice_expiry() {
    let project = |name: &str, field: &str, raw: serde_json::Value| {
        let mut value = fixture(name);
        value[field] = raw;
        Swap::from(serde_json::from_value::<GetSwapResponse>(value).unwrap())
    };
    for name in [
        "lightning_to_arkade",
        "arkade_to_lightning",
        "evm_to_lightning",
        "lightning_to_evm",
        "bitcoin_to_lightning",
    ] {
        let swap = project(name, "invoice_expires_at", json!(0));
        assert_eq!(swap.invoice_expires_at, None, "{name}");
    }
    for name in [
        "lightning_to_arkade",
        "arkade_to_lightning",
        "evm_to_arkade",
    ] {
        let swap = project(name, "vhtlc_refund_locktime", json!(u64::MAX));
        assert_eq!(swap.vhtlc_refund_locktime, None, "{name}");
    }
    let swap = project("evm_to_arkade", "evm_refund_locktime", json!(u64::MAX));
    assert_eq!(swap.evm_refund_locktime, None);
}

#[tokio::test]
async fn lightning_receive_uses_current_amount_fields_script_version_and_fees() {
    for amount in [QuoteAmount::Source(100_000), QuoteAmount::Target(100_000)] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/swap/lightning/arkade"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(hold_invoice_response("lightning_to_arkade", 0)),
            )
            .expect(1)
            .mount(&server)
            .await;
        let swap = client(&server)
            .create_lightning_to_arkade_swap_with_options(
                amount.clone(),
                Address::Arkade("tark1qqellv77udfmr20tun8dvju5vgudpf9vxe8jwhthrkn26fz96pawqfdy8nk05rsmrf8h94j26905e7n6sng8y059z8ykn2j5xcuw4xt846qj6x".into()),
                Some(25),
                Some("Invoice #42".into()),
            )
            .await
            .unwrap();
        assert_eq!(swap.status, SwapStatus::Pending);
        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_schema(&body, "LightningToArkadeSwapRequest");
        let (present, absent) = match amount {
            QuoteAmount::Source(_) => ("source_amount_sats", "target_amount_sats"),
            QuoteAmount::Target(_) => ("target_amount_sats", "source_amount_sats"),
        };
        assert_eq!(body[present], 100_000);
        assert!(body.get(absent).is_none());
        assert!(body.get("sats_receive").is_none());
        assert_eq!(body["arkade_htlc_script_version"], 1);
        assert_eq!(body["extra_fees"], 25);
        assert_eq!(body["invoice_description"], "Invoice #42");
    }
}

#[tokio::test]
async fn arkade_send_uses_current_destination_fields() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/swap/arkade/lightning"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("arkade_to_lightning")))
        .expect(2)
        .mount(&server)
        .await;
    for destination in [
        LightningDestination::Address {
            address: "user@example.com".into(),
            sats: 100_000,
        },
        LightningDestination::LnurlSendMax {
            lnurl: "lnurl1fixture".into(),
            amount: 100_000,
        },
    ] {
        client(&server)
            .create_arkade_to_lightning_swap_with_options(destination, Some(12))
            .await
            .unwrap();
    }
    let requests = server.received_requests().await.unwrap();
    for request in &requests {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_schema(&body, "ArkadeToLightningSwapRequest");
        assert_eq!(body["arkade_htlc_script_version"], 1);
        assert_eq!(body["extra_fees"], 12);
        assert!(body.get("amount_sats").is_none());
    }
    let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let second: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(first["target_amount_sats"], 100_000);
    assert_eq!(second["source_amount_sats"], 100_000);
}

#[tokio::test]
async fn duplicate_invoice_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/swap/arkade/lightning"))
        .respond_with(
            ResponseTemplate::new(409).set_body_json(json!({"error":"preimage already exists"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (client, records) = recording_client(&server);
    let err = client
        .create_arkade_to_lightning_swap(LightningDestination::Invoice(invoice("lnd_hold_22")))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Api { status: 409, .. }));
    let records = records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].event.as_deref(),
        Some("create.retry.attempt_failed")
    );
    assert_eq!(records[0].data["attempt"], json!(1));
    assert!(
        records[0]
            .error
            .as_deref()
            .unwrap()
            .contains("preimage already exists")
    );
}

#[tokio::test]
async fn expected_invoice_amount_reaches_creation_and_quote() {
    let server = MockServer::start().await;
    for (route, direction) in [
        ("/swap/arkade/lightning", "arkade_to_lightning"),
        ("/swap/evm/lightning", "evm_to_lightning"),
    ] {
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(created(direction)))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/quote/lightning-send"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "exchange_rate":"60000", "source_amount":"1010", "source_amount_sats":1_010,
            "target_amount_sats":1_000, "protocol_fee_sats":5, "network_fee_sats":5,
            "protocol_fee_rate":0.001, "min_amount_sats":100, "max_amount_sats":1_000_000
        })))
        .expect(1)
        .mount(&server)
        .await;
    let invoice = invoice("lnd_hold_22");
    let destination = || LightningDestination::InvoiceExpecting {
        invoice: invoice.clone(),
        sats: 1_000,
    };
    // The two fixtures share a swap id, so each creation needs its own storage.
    client(&server)
        .create_arkade_to_lightning_swap(destination())
        .await
        .unwrap();
    client(&server)
        .create_evm_to_lightning_swap(TokenId::Usdt0Arbitrum, destination(), false, None)
        .await
        .unwrap();
    client(&server)
        .get_lightning_send_quote(
            lendaswap_sdk::types::KnownChain::Arkade,
            TokenId::Btc,
            destination(),
            None,
        )
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    for (request, schema) in requests[..2]
        .iter()
        .zip(["ArkadeToLightningSwapRequest", "EvmToLightningSwapRequest"])
    {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_schema(&body, schema);
        assert_eq!(body["lightning_invoice"], invoice);
        assert_eq!(body["target_amount_sats"], 1_000);
        assert!(body.get("source_amount_sats").is_none());
        assert!(body.get("amount_in").is_none());
    }
    let query: std::collections::HashMap<_, _> =
        requests[2].url.query_pairs().into_owned().collect();
    assert_eq!(query["lightning_invoice"], invoice);
    assert_eq!(query["target_amount_sats"], "1000");
    let wire = serde_json::to_value(
        lendaswap_sdk::types::CreateArkadeToLightningSwapRequest::new(
            destination(),
            "02aa",
            "02bb",
            None,
        ),
    )
    .unwrap();
    assert_eq!(wire["lightning_invoice"], invoice);
    assert_eq!(wire["target_amount_sats"], 1_000);
}

#[tokio::test]
async fn get_swap_sends_compatibility_headers_for_every_direction() {
    let server = MockServer::start().await;
    for direction in [
        "btc_to_arkade",
        "lightning_to_arkade",
        "arkade_to_lightning",
        "lightning_to_evm",
        "bitcoin_to_evm",
        "arkade_to_evm",
        "evm_to_arkade",
        "evm_to_bitcoin",
        "evm_to_lightning",
    ] {
        let mut response = fixture(direction);
        response["id"] = json!(direction);
        Mock::given(method("GET"))
            .and(path(format!("/swap/{direction}")))
            .and(header("x-satora-bitcoin-htlc-version", "1"))
            .and(header("x-satora-arkade-vhtlc-version", "1"))
            .and(header("x-satora-lightning-version", "1"))
            .and(header("x-satora-evm-erc20-htlc-version", "1"))
            .and(header("x-satora-evm-native-htlc-version", "1"))
            .and(header(
                "X-Lendaswap-Client",
                concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            client(&server).get_swap(direction).await.unwrap().status,
            SwapStatus::Pending
        );
    }
}

#[tokio::test]
async fn polling_deadline_cancels_a_stalled_http_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/swap/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(fixture("evm_to_arkade"))
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        client(&server).wait_for_swap_status(
            "slow",
            &[SwapStatus::ServerFunded],
            Duration::from_millis(50),
        ),
    )
    .await
    .expect("the SDK must enforce its own shorter deadline");
    assert!(matches!(result, Err(Error::Timeout(_))));
}

#[tokio::test]
async fn polling_deadline_includes_sleep_between_requests() {
    let server = MockServer::start().await;
    let mut response = fixture("evm_to_arkade");
    response["id"] = json!("pending");
    Mock::given(method("GET"))
        .and(path("/swap/pending"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .expect(1)
        .mount(&server)
        .await;
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        client(&server).wait_for_swap_status(
            "pending",
            &[SwapStatus::ServerFunded],
            Duration::from_millis(50),
        ),
    )
    .await
    .expect("polling sleep must not exceed the timeout");
    assert!(matches!(result, Err(Error::Timeout(_))));
}

#[tokio::test]
async fn unhealthy_status_is_data_and_unknown_bulk_ids_are_preserved() {
    let server = MockServer::start().await;
    Mock::given(path("/status"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({
            "healthy":false,"swaps_paused":true,"services":{
                "arkade":{"healthy":false,"error":"unavailable"},"bitcoin":{"healthy":true},
                "ethereum":{"healthy":true},"polygon":{"healthy":true},"arbitrum":{"healthy":true}
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/swap/bulk-status"))
        .and(body_partial_json(json!({"ids":["missing"]})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"statuses":[],"not_found":["missing"]})),
        )
        .mount(&server)
        .await;
    let client = client(&server);
    assert!(!client.get_status().await.unwrap().healthy);
    assert_eq!(
        client
            .get_bulk_status(&["missing".into()])
            .await
            .unwrap()
            .not_found,
        vec!["missing"]
    );
}

fn recovered(index: u32) -> Value {
    let mut value = fixture("evm_to_arkade");
    value["id"] = json!(format!("swap-{index}"));
    value["index"] = json!(index);
    value["hash_lock"] = json!(hex::encode(
        Signer::from_mnemonic(MNEMONIC)
            .unwrap()
            .derive_swap_params(index)
            .unwrap()
            .hash_lock
    ));
    value
}

#[tokio::test]
async fn recovery_pages_restore_keys_and_never_regress_the_counter() {
    let server = MockServer::start().await;
    for (start, index, until, complete) in [(0, 3, 9, false), (10, 12, 20, true)] {
        Mock::given(method("POST")).and(path("/swap/recover"))
            .and(body_partial_json(json!({"start_index":start,"xpub":Signer::from_mnemonic(MNEMONIC).unwrap().derive_user_id_xpub().unwrap().to_string()})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "swaps":[recovered(index)],"highest_index":index,"next_index":index+1,"scanned_until":until,"complete":complete
            }))).expect(1).mount(&server).await;
    }
    let storage = Arc::new(InMemorySwapStorage::new());
    storage.ensure_next_key_index(50).unwrap();
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = records.clone();
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(storage.clone())
        .logger(Arc::new(move |record: &LogRecord| {
            sink.lock().unwrap().push(record.clone())
        }))
        .log_level(LogLevel::Debug)
        .build()
        .unwrap();
    let recovery = client.recover_all_swaps(None).await.unwrap();
    assert_eq!(recovery.swaps.len(), 2);
    assert!(recovery.complete);
    let records = records.lock().unwrap();
    let events: Vec<_> = records.iter().filter_map(|r| r.event.as_deref()).collect();
    assert_eq!(
        events,
        [
            "client.recover.start",
            "client.recover.response",
            "client.recover.start",
            "client.recover.response",
        ]
    );
    let xpub = Signer::from_mnemonic(MNEMONIC)
        .unwrap()
        .derive_user_id_xpub()
        .unwrap()
        .to_string();
    assert_eq!(records[0].level, LogLevel::Info);
    assert_eq!(
        Value::Object(records[0].data.clone()),
        json!({"xpub": xpub, "startIndex": null})
    );
    assert_eq!(records[2].data["startIndex"], json!(10));
    assert_eq!(records[1].level, LogLevel::Debug);
    assert_eq!(
        Value::Object(records[1].data.clone()),
        json!({
            "recoveredSwapCount": 1,
            "highestIndex": 3,
            "nextIndex": 4,
            "scannedUntil": 9,
            "complete": false,
        })
    );
    assert_eq!(storage.get_swap_key_index("swap-3").unwrap(), Some(3));
    assert_eq!(storage.get_swap_key_index("swap-12").unwrap(), Some(12));
    assert_eq!(storage.next_key_index().unwrap(), 50);
}

#[tokio::test]
async fn recovery_rejects_a_cursor_outside_the_scanned_range() {
    for (start, scanned_until, next_index) in [
        (0, 49, 1u32 << 31),
        (0, (1u32 << 31) - 1, 1 << 31),
        (0, 49, 51),
        (10, 59, 5),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/swap/recover"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "swaps":[],"highest_index":0,"next_index":next_index,"scanned_until":scanned_until,"complete":true
            })))
            .mount(&server)
            .await;
        let storage = Arc::new(InMemorySwapStorage::new());
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(storage.clone())
            .build()
            .unwrap();
        assert!(
            matches!(
                client.recover_swaps(Some(start)).await,
                Err(Error::Decode(_))
            ),
            "start {start}, scanned_until {scanned_until}, next_index {next_index}"
        );
        assert_eq!(storage.next_key_index().unwrap(), 0);
    }
}

#[tokio::test]
async fn recovery_honors_a_cursor_past_skipped_legacy_swaps() {
    let server = MockServer::start().await;
    Mock::given(path("/swap/recover"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "swaps":[],"highest_index":0,"next_index":50,"scanned_until":99,"complete":true
        })))
        .mount(&server)
        .await;
    let storage = Arc::new(InMemorySwapStorage::new());
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(storage.clone())
        .build()
        .unwrap();
    client.recover_swaps(None).await.unwrap();
    assert_eq!(storage.next_key_index().unwrap(), 50);
}

#[tokio::test]
async fn recovery_rejects_a_wrong_wallet_before_mutating_storage() {
    let server = MockServer::start().await;
    let mut swap = recovered(3);
    swap["hash_lock"] = json!("00".repeat(32));
    Mock::given(path("/swap/recover"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "swaps":[recovered(2), swap],"highest_index":3,"next_index":4,"scanned_until":10,"complete":true
        })))
        .mount(&server)
        .await;
    let storage = Arc::new(InMemorySwapStorage::new());
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(storage.clone())
        .build()
        .unwrap();
    match client.recover_swaps(None).await {
        Err(Error::InvalidSwap(message)) => assert!(
            message.contains("swap-3 does not match this wallet"),
            "{message}"
        ),
        other => panic!("expected a wallet mismatch, got {other:?}"),
    }
    // The valid swap on the same page is not persisted either.
    assert_eq!(storage.get_swap_key_index("swap-2").unwrap(), None);
    assert_eq!(storage.get_swap_key_index("swap-3").unwrap(), None);
    assert_eq!(storage.next_key_index().unwrap(), 0);
}

#[tokio::test]
async fn recovery_and_get_swap_read_a_bitcoin_to_lightning_swap() {
    let server = MockServer::start().await;
    let response = fixture("bitcoin_to_lightning");
    let id = response["id"].as_str().unwrap().to_owned();
    let mut swap = response.clone();
    swap["index"] = json!(3);
    Mock::given(path("/swap/recover"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "swaps":[recovered(2), swap],"highest_index":3,"next_index":4,"scanned_until":10,"complete":true
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/swap/{id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&server)
        .await;
    let storage = Arc::new(InMemorySwapStorage::new());
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(storage.clone())
        .build()
        .unwrap();
    client.recover_swaps(None).await.unwrap();
    assert_eq!(storage.get_swap_key_index("swap-2").unwrap(), Some(2));
    assert_eq!(storage.get_swap_key_index(&id).unwrap(), Some(3));
    let swap = client.get_swap(&id).await.unwrap();
    assert_eq!(swap.direction, SwapDirection::BitcoinToLightning);
    assert!(matches!(
        &swap.funding,
        SwapFunding::BitcoinAddress { address } if address == "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080"
    ));
    assert_eq!(swap.deposit_amount, "1253");
    assert_eq!(swap.receive_amount, "1000");
}

#[tokio::test]
async fn recovery_refuses_a_bitcoin_to_lightning_swap_whose_invoice_does_not_pay_its_hash_lock() {
    let server = MockServer::start().await;
    let mut swap = fixture("bitcoin_to_lightning");
    let id = swap["id"].as_str().unwrap().to_owned();
    swap["index"] = json!(3);
    swap["hash_lock"] = json!(format!("0x{}", "33".repeat(32)));
    Mock::given(path("/swap/recover"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "swaps":[recovered(2), swap],"highest_index":3,"next_index":4,"scanned_until":10,"complete":true
        })))
        .mount(&server)
        .await;
    let storage = Arc::new(InMemorySwapStorage::new());
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(storage.clone())
        .build()
        .unwrap();
    let result = client.recover_swaps(None).await;
    assert!(is_invoice_hash_mismatch(&result), "{result:?}");
    assert_eq!(storage.get_swap_key_index("swap-2").unwrap(), None);
    assert_eq!(storage.get_swap_key_index(&id).unwrap(), None);
    assert_eq!(storage.next_key_index().unwrap(), 0);
}

#[tokio::test]
async fn persistent_client_reopens_the_same_wallet_namespace() {
    let server = MockServer::start().await;
    Mock::given(path("/swap/lightning/arkade"))
        .respond_with({
            let next_id = std::sync::atomic::AtomicU32::new(0);
            move |request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let hash_lock = body["hash_lock"].as_str().unwrap();
                let mut response = fixture("lightning_to_arkade");
                response["hash_lock"] = json!(hash_lock);
                response["bolt11_invoice"] = json!(invoice_paying(hash_lock));
                response["id"] = json!(format!(
                    "swap-{}",
                    next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ));
                ResponseTemplate::new(200).set_body_json(response)
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("swaps.db");
    for _ in 0..2 {
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage_path(&path)
            .build()
            .unwrap();
        client
            .create_lightning_to_arkade_swap(100_000, Address::Arkade("tark1qqellv77udfmr20tun8dvju5vgudpf9vxe8jwhthrkn26fz96pawqfdy8nk05rsmrf8h94j26905e7n6sng8y059z8ykn2j5xcuw4xt846qj6x".into()))
            .await
            .unwrap();
    }
    let requests = server.received_requests().await.unwrap();
    let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let second: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_ne!(first["hash_lock"], second["hash_lock"]);
    let wallet_id = Signer::from_mnemonic(MNEMONIC)
        .unwrap()
        .derive_user_id_xpub()
        .unwrap()
        .to_string();
    let storage = lendaswap_sdk::SqliteSwapStorage::open(path, &wallet_id).unwrap();
    assert_eq!(storage.get_swap_key_index("swap-0").unwrap(), Some(0));
    assert_eq!(storage.get_swap_key_index("swap-1").unwrap(), Some(1));
}

#[test]
fn debug_output_redacts_seeds_private_keys_and_preimages() {
    let signer = Signer::from_mnemonic(MNEMONIC).unwrap();
    let params = signer.derive_swap_params(0).unwrap();
    let evm = signer.derive_evm_key(0).unwrap();
    assert!(!format!("{signer:?}").contains(MNEMONIC));
    assert!(!format!("{params:?}").contains(&format!("{:?}", params.secret)));
    assert!(!format!("{params:?}").contains(&format!("{:?}", params.preimage)));
    assert!(!format!("{evm:?}").contains(&format!("{:?}", evm.secret_key)));
    let client = Client::builder().mnemonic(MNEMONIC).build().unwrap();
    assert!(!format!("{client:?}").contains(MNEMONIC));
}

#[tokio::test]
async fn oversized_btc_amounts_return_errors_without_panicking() {
    let server = MockServer::start().await;
    let client = client(&server);
    assert!(matches!(
        client
            .create_evm_to_arkade_swap(
                TokenId::UsdcPolygon,
                QuoteAmount::Target(u128::MAX),
                Address::Arkade("tark1qqellv77udfmr20tun8dvju5vgudpf9vxe8jwhthrkn26fz96pawqfdy8nk05rsmrf8h94j26905e7n6sng8y059z8ykn2j5xcuw4xt846qj6x".into()),
                true,
                None
            )
            .await,
        Err(Error::InvalidSwap(_))
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn evm_lightning_creation_keeps_send_max_amounts_lossless() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/swap/evm/lightning"))
        .respond_with(ResponseTemplate::new(200).set_body_json(created("evm_to_lightning")))
        .expect(1)
        .mount(&server)
        .await;
    let storage = Arc::new(InMemorySwapStorage::new());
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(storage.clone())
        .referral_code("merchant")
        .build()
        .unwrap();
    client
        .create_evm_to_lightning_swap(
            TokenId::Usdt0Arbitrum,
            LightningDestination::AddressSendMax {
                address: "user@example.com".into(),
                amount: u128::from(u64::MAX) + 1,
            },
            true,
            Some(10),
        )
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_schema(&body, "EvmToLightningSwapRequest");
    assert_eq!(body["amount_in"], "18446744073709551616");
    assert!(body.get("target_amount_sats").is_none());
    assert!(body.get("hash_lock").is_none());
    assert_eq!(body["referral_code"], "merchant");
    assert_eq!(
        body["user_address"],
        Signer::from_mnemonic(MNEMONIC)
            .unwrap()
            .derive_evm_key(0)
            .unwrap()
            .address
    );
    assert_eq!(
        storage
            .get_swap_key_index("00000000-0000-0000-0000-000000000001")
            .unwrap(),
        Some(0)
    );
}

#[tokio::test]
async fn exact_lightning_quote_encodes_destination_source_and_referral() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/quote/lightning-send"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "exchange_rate":"60000", "source_amount":"123456", "source_amount_sats":100_010,
            "target_amount_sats":100_000, "protocol_fee_sats":5, "network_fee_sats":5,
            "protocol_fee_rate":0.001, "min_amount_sats":100, "max_amount_sats":1_000_000
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .referral_code("merchant")
        .build()
        .unwrap();
    let quote = client
        .get_lightning_send_quote(
            lendaswap_sdk::types::KnownChain::Arbitrum,
            TokenId::Usdt0Arbitrum,
            LightningDestination::Lnurl {
                lnurl: "lnurl1fixture".into(),
                sats: 100_000,
            },
            Some(10),
        )
        .await
        .unwrap();
    assert_eq!(quote.source_amount, "123456");
    let requests = server.received_requests().await.unwrap();
    let query: std::collections::HashMap<_, _> =
        requests[0].url.query_pairs().into_owned().collect();
    assert_eq!(query["source_chain"], "42161");
    assert_eq!(query["target_amount_sats"], "100000");
    assert_eq!(query["ref"], "merchant");
    assert_eq!(query["extra_fees"], "10");
    assert!(!query.contains_key("source_amount"));
}

#[tokio::test]
async fn recovery_rejects_a_non_advancing_cursor() {
    let server = MockServer::start().await;
    Mock::given(path("/swap/recover"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "swaps":[],"highest_index":0,"next_index":0,"scanned_until":1,"complete":false
        })))
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        client(&server).recover_all_swaps(Some(10)).await,
        Err(Error::Decode(_))
    ));
}

#[tokio::test]
async fn quote_uses_client_referral_unless_the_request_overrides_it() {
    let server = MockServer::start().await;
    Mock::given(path("/quote"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error":"test"})))
        .expect(2)
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .referral_code("default")
        .build()
        .unwrap();
    for code in [None, Some("override".to_string())] {
        let mut request = lendaswap_sdk::types::QuoteRequest::new(
            lendaswap_sdk::types::Chain::lightning(),
            TokenId::Btc,
            lendaswap_sdk::types::Chain::arkade(),
            TokenId::Btc,
            QuoteAmount::Source(100_000),
        );
        request.referral_code = code;
        request.extra_fees_bps = Some(20);
        assert!(matches!(
            client.get_quote(request).await,
            Err(Error::Api { status: 400, .. })
        ));
    }
    let requests = server.received_requests().await.unwrap();
    for (request, expected) in requests.iter().zip(["default", "override"]) {
        let query: std::collections::HashMap<_, _> =
            request.url.query_pairs().into_owned().collect();
        assert_eq!(query["ref"], expected);
        assert_eq!(query["extra_fees"], "20");
    }
}

#[cfg(feature = "gasless")]
#[tokio::test]
async fn deposit_polling_deadline_includes_fetching_the_swap() {
    let server = MockServer::start().await;
    Mock::given(path("/swap/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(fixture("evm_to_lightning"))
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        client(&server).wait_for_deposit_funding(
            "slow",
            &url::Url::parse(&server.uri()).unwrap(),
            0,
            Duration::from_millis(50),
        ),
    )
    .await
    .expect("setup must obey the deposit polling timeout");
    assert!(matches!(result, Err(Error::Timeout(_))));
}

#[cfg(feature = "gasless")]
#[tokio::test]
async fn deposit_probe_accepts_evm_lightning_and_rejects_malformed_rpc_balances() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/swap/deposit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("evm_to_lightning")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"method":"eth_getBalance"})))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":"0x0"}))
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"method":"eth_call"})))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":"0x"}))
        })
        .mount(&server)
        .await;
    let result = client(&server)
        .check_deposit_funding("deposit", Some(url::Url::parse(&server.uri()).unwrap()))
        .await;
    assert!(matches!(result, Err(Error::Decode(_))), "got {result:?}");
}

#[tokio::test]
async fn dispatcher_forwards_lightning_source_amount_and_surcharge() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/swap/lightning/arkade"))
        .and(body_partial_json(
            json!({"source_amount_sats":100_000,"extra_fees":15}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(hold_invoice_response("lightning_to_arkade", 0)),
        )
        .expect(1)
        .mount(&server)
        .await;
    client(&server)
        .create_swap(
            lendaswap_sdk::types::KnownChain::Lightning,
            TokenId::Btc,
            TokenId::Btc,
            QuoteAmount::Source(100_000),
            Some(Address::Arkade("tark1qqellv77udfmr20tun8dvju5vgudpf9vxe8jwhthrkn26fz96pawqfdy8nk05rsmrf8h94j26905e7n6sng8y059z8ykn2j5xcuw4xt846qj6x".into())),
            Some(15),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn bitcoin_receive_helper_creates_a_bitcoin_to_arkade_swap() {
    let arkade = "tark1qqellv77udfmr20tun8dvju5vgudpf9vxe8jwhthrkn26fz96pawqfdy8nk05rsmrf8h94j26905e7n6sng8y059z8ykn2j5xcuw4xt846qj6x";
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/swap/bitcoin/arkade"))
        .and(body_partial_json(json!({
            "sats_receive": 100_000,
            "target_arkade_address": arkade,
            "extra_fees": 15,
        })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(bitcoin_htlc_response("btc_to_arkade", 0)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server);
    let err = client
        .create_bitcoin_to_arkade_swap(100_000, Address::Bitcoin("bcrt1qfixture".into()), None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidSwap(_)), "{err:?}");
    let swap = client
        .create_bitcoin_to_arkade_swap(100_000, Address::Arkade(arkade.into()), Some(15))
        .await
        .unwrap();
    assert_eq!(swap.direction, SwapDirection::BtcToArkade);
    assert!(
        matches!(swap.funding, SwapFunding::BitcoinAddress { .. }),
        "{:?}",
        swap.funding
    );
}

#[tokio::test]
async fn bitcoin_creation_refuses_an_htlc_address_our_keys_do_not_rebuild() {
    let arkade = Address::Arkade("tark1qqellv77udfmr20tun8dvju5vgudpf9vxe8jwhthrkn26fz96pawqfdy8nk05rsmrf8h94j26905e7n6sng8y059z8ykn2j5xcuw4xt846qj6x".into());
    let (_, _, _, bitcoin_to_evm) = creation_cases(QuoteAmount::Target(100_000))
        .into_iter()
        .find(|case| case.1 == "bitcoin_to_evm")
        .unwrap();
    for (route, direction, entry) in [
        (
            "bitcoin/arkade",
            "btc_to_arkade",
            "create_bitcoin_to_arkade_swap",
        ),
        ("bitcoin/arkade", "btc_to_arkade", "create_swap"),
        ("bitcoin/evm", "bitcoin_to_evm", "create_swap_request"),
    ] {
        let mut response = bitcoin_htlc_response(direction, 0);
        response["btc_htlc_address"] =
            bitcoin_htlc_response(direction, 1)["btc_htlc_address"].clone();
        let server = MockServer::start().await;
        Mock::given(path(format!("/swap/{route}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        let client = client(&server);
        let refused = match entry {
            "create_swap" => {
                client
                    .create_swap(
                        lendaswap_sdk::types::KnownChain::Bitcoin,
                        TokenId::Btc,
                        TokenId::Btc,
                        QuoteAmount::Target(100_000),
                        Some(arkade.clone()),
                        None,
                    )
                    .await
            }
            "create_swap_request" => {
                client
                    .create_swap_request(bitcoin_to_evm.clone(), Default::default())
                    .await
            }
            _ => {
                client
                    .create_bitcoin_to_arkade_swap(100_000, arkade.clone(), None)
                    .await
            }
        }
        .unwrap_err();
        assert!(
            matches!(&refused, Error::InvalidSwap(message) if message.contains("address mismatch")),
            "{entry}: {refused:?}"
        );
    }
}

#[tokio::test]
async fn discovery_endpoints_decode_the_current_schema() {
    let server = MockServer::start().await;
    let responses = [
        (
            "/tokens",
            "TokenInfos",
            json!({"btc_tokens":[],"evm_tokens":[]}),
        ),
        (
            "/swap-pairs",
            "SwapPairsResponse",
            json!({"pairs":[{
                "source":"Lightning","target":"Arkade","min_sats":100,"max_sats":100000,
                "fee_percentage":0.003,"network_fee_sats":10
            }]}),
        ),
        (
            "/chain-config",
            "ChainConfigResponse",
            json!({"chains":[{
                "chain":"42161","btc_peg_rate":"1","btc_pegged_token":{
                    "address":"0x1234","decimals":18,"symbol":"tBTC"
                }
            }]}),
        ),
        (
            "/network-fees",
            "NetworkFeesResponse",
            json!({"pairs":[{
                "source":"Arkade","target":"137","fees":{"source_sats":0,"target_sats":50}
            }]}),
        ),
        (
            "/mtp",
            "MtpResponse",
            json!({"mtp":1_700_000_000,"tip_height":800_000}),
        ),
        (
            "/support-agents",
            "SupportAgentsResponse",
            json!({"agents":[{"npub":"npub1fixture"}]}),
        ),
        (
            "/referral-fee",
            "ReferralFeeResponse",
            json!({"extra_fee_rate":0.001,"referral_label":"merchant"}),
        ),
    ];
    for (route, schema, response) in &responses {
        assert_schema(response, schema);
        Mock::given(method("GET"))
            .and(path(*route))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
    }
    let client = client(&server);
    let results = [
        serde_json::to_value(client.get_tokens().await.unwrap()).unwrap(),
        serde_json::to_value(client.get_swap_pairs().await.unwrap()).unwrap(),
        serde_json::to_value(client.get_chain_config().await.unwrap()).unwrap(),
        serde_json::to_value(client.get_network_fees().await.unwrap()).unwrap(),
        serde_json::to_value(client.get_mtp().await.unwrap()).unwrap(),
        json!({"agents":client.get_support_agents().await.unwrap()}),
        serde_json::to_value(
            client
                .get_referral_fee(Some("merchant"), Some(10))
                .await
                .unwrap(),
        )
        .unwrap(),
    ];
    for ((_, schema, _), result) in responses.iter().zip(results) {
        assert_schema(&result, schema);
    }
}

#[tokio::test]
async fn recovery_uses_hash160_for_bitcoin_to_arkade_swaps() {
    use bitcoin::hashes::Hash;
    use bitcoin::hashes::hash160;
    let server = MockServer::start().await;
    let params = Signer::from_mnemonic(MNEMONIC)
        .unwrap()
        .derive_swap_params(3)
        .unwrap();
    let mut swap = fixture("btc_to_arkade");
    swap["index"] = json!(3);
    swap["hash_lock"] = json!(hash160::Hash::hash(&params.preimage).to_string());
    Mock::given(path("/swap/recover"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "swaps":[swap], "highest_index":3, "next_index":4,
            "scanned_until":10, "complete":true
        })))
        .mount(&server)
        .await;
    assert_eq!(
        client(&server)
            .recover_swaps(None)
            .await
            .unwrap()
            .swaps
            .len(),
        1
    );
}

fn creation_cases(
    amount: QuoteAmount,
) -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    lendaswap_sdk::types::SwapCreation,
)> {
    use lendaswap_sdk::types::EvmAsset;
    use lendaswap_sdk::types::SwapCreation::*;
    let asset = EvmAsset {
        chain_id: 137,
        token: TokenId::Other("0x1111111111111111111111111111111111111111".into()),
    };
    let arkade = "tark1qqellv77udfmr20tun8dvju5vgudpf9vxe8jwhthrkn26fz96pawqfdy8nk05rsmrf8h94j26905e7n6sng8y059z8ykn2j5xcuw4xt846qj6x".to_owned();
    let target_address = "0x1111111111111111111111111111111111111111".to_owned();
    let destination = match &amount {
        QuoteAmount::Source(v) => LightningDestination::AddressSendMax {
            address: "merchant@example.com".into(),
            amount: *v,
        },
        QuoteAmount::Target(v) => LightningDestination::Address {
            address: "merchant@example.com".into(),
            sats: u64::try_from(*v).unwrap(),
        },
    };
    vec![
        (
            "bitcoin/arkade",
            "btc_to_arkade",
            "BitcoinToArkadeSwapRequest",
            BitcoinToArkade {
                sats_receive: 100_000,
                target_address: arkade.clone(),
            },
        ),
        (
            "lightning/arkade",
            "lightning_to_arkade",
            "LightningToArkadeSwapRequest",
            LightningToArkade {
                amount: amount.clone(),
                target_address: arkade.clone(),
            },
        ),
        (
            "arkade/lightning",
            "arkade_to_lightning",
            "ArkadeToLightningSwapRequest",
            ArkadeToLightning {
                destination: destination.clone(),
            },
        ),
        (
            "bitcoin/evm",
            "bitcoin_to_evm",
            "BitcoinToEvmSwapRequest",
            BitcoinToEvm {
                target: asset.clone(),
                amount: amount.clone(),
                target_address: target_address.clone(),
            },
        ),
        (
            "arkade/evm",
            "arkade_to_evm",
            "ArkadeToEvmSwapRequest",
            ArkadeToEvm {
                target: asset.clone(),
                amount: amount.clone(),
                target_address: target_address.clone(),
            },
        ),
        (
            "lightning/evm",
            "lightning_to_evm",
            "LightningToEvmSwapRequest",
            LightningToEvm {
                target: asset.clone(),
                amount: amount.clone(),
                target_address,
            },
        ),
        (
            "evm/bitcoin",
            "evm_to_bitcoin",
            "EvmToBitcoinSwapRequest",
            EvmToBitcoin {
                source: asset.clone(),
                amount: amount.clone(),
                target_address: Some("1BoatSLRHtKNngkdXEeobR76b53LETtpyT".into()),
            },
        ),
        (
            "evm/arkade",
            "evm_to_arkade",
            "EvmToArkadeGenericSwapRequest",
            EvmToArkade {
                source: asset.clone(),
                amount: amount.clone(),
                target_address: arkade,
            },
        ),
        (
            "evm/lightning",
            "evm_to_lightning",
            "EvmToLightningSwapRequest",
            EvmToLightning {
                source: asset,
                destination,
            },
        ),
    ]
}

#[tokio::test]
async fn creates_all_nine_directions_with_current_schemas_and_persists_snapshots() {
    use bitcoin::hashes::Hash;
    use bitcoin::hashes::hash160;
    use lendaswap_sdk::types::SwapOptions;
    for amount in [QuoteAmount::Source(100_000), QuoteAmount::Target(100_000)] {
        for (route, direction, schema, request) in creation_cases(amount) {
            let server = MockServer::start().await;
            let response = match direction {
                "btc_to_arkade" | "bitcoin_to_evm" => bitcoin_htlc_response(direction, 0),
                "lightning_to_arkade" | "lightning_to_evm" => hold_invoice_response(direction, 0),
                _ => created(direction),
            };
            Mock::given(method("POST"))
                .and(path(format!("/swap/{route}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(response))
                .expect(1)
                .mount(&server)
                .await;
            let client = client(&server);
            let swap = client
                .create_swap_request(
                    request,
                    SwapOptions {
                        extra_fees_bps: Some(25),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            let requests = server.received_requests().await.unwrap();
            let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
            assert_schema(&body, schema);
            assert_eq!(body["extra_fees"], 25);
            let stored = client.get_stored_swap(&swap.id).unwrap().unwrap();
            assert_eq!(stored.response.id(), swap.id);
            let params = Signer::from_mnemonic(MNEMONIC)
                .unwrap()
                .derive_swap_params(0)
                .unwrap();
            if route == "bitcoin/arkade" {
                assert_eq!(
                    body["hash_lock"],
                    hash160::Hash::hash(&params.preimage).to_string()
                );
                assert_eq!(body["claim_pk"], body["refund_pk"]);
            } else if !route.ends_with("/lightning") {
                assert_eq!(
                    body["hash_lock"],
                    format!("0x{}", hex::encode(params.hash_lock))
                );
            } else {
                assert!(body.get("hash_lock").is_none());
            }
        }
    }
}

#[tokio::test]
async fn evm_sourced_creation_refuses_a_deposit_address_it_did_not_send() {
    let own = Signer::from_mnemonic(MNEMONIC)
        .unwrap()
        .derive_evm_key(0)
        .unwrap()
        .address;
    let upper_case = format!("0x{}", own[2..].to_uppercase());
    for (route, direction, _, request) in creation_cases(QuoteAmount::Source(100_000)) {
        if !direction.starts_with("evm_") {
            continue;
        }
        for (returned, refused) in [
            ("0x3333333333333333333333333333333333333333", true),
            (upper_case.as_str(), false),
        ] {
            let server = MockServer::start().await;
            let mut response = fixture(direction);
            response["client_evm_address"] = json!(returned);
            response["gasless"] = json!(true);
            let id = response["id"].as_str().unwrap().to_owned();
            Mock::given(method("POST"))
                .and(path(format!("/swap/{route}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(response))
                .expect(1)
                .mount(&server)
                .await;
            let client = client(&server);
            let result = client
                .create_swap_request(request.clone(), Default::default())
                .await;
            let body: Value =
                serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
            assert_eq!(body["user_address"], json!(own), "{direction}");
            if refused {
                assert!(
                    matches!(&result, Err(Error::InvalidSwap(message)) if message.contains("deposit address")),
                    "{direction}: {result:?}"
                );
                let stored = client.get_stored_swap(&id).unwrap().expect(direction);
                assert_eq!(
                    serde_json::to_value(&stored.response).unwrap()["client_evm_address"],
                    json!(returned),
                    "{direction}"
                );
            } else {
                let funding = result.unwrap().funding;
                assert_eq!(
                    funding,
                    SwapFunding::Gasless {
                        deposit_address: upper_case.clone()
                    },
                    "{direction}"
                );
            }
        }
    }
}

#[tokio::test]
async fn creation_preserves_large_evm_amounts_and_bridge_execution_metadata() {
    use lendaswap_sdk::types::BridgeRoute;
    use lendaswap_sdk::types::EvmAsset;
    use lendaswap_sdk::types::SwapCreation;
    use lendaswap_sdk::types::SwapOptions;
    let server = MockServer::start().await;
    Mock::given(path("/swap/lightning/evm"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(hold_invoice_response("lightning_to_evm", 0)),
        )
        .mount(&server)
        .await;
    let client = client(&server);
    let request = SwapCreation::LightningToEvm {
        target: EvmAsset {
            chain_id: 137,
            token: TokenId::Other("0x1111111111111111111111111111111111111111".into()),
        },
        amount: QuoteAmount::Target(u128::MAX),
        target_address: "0x2222222222222222222222222222222222222222".into(),
    };
    let swap = client
        .create_swap_request(
            request,
            SwapOptions {
                invoice_description: Some("Invoice #42".into()),
                bridge: Some(BridgeRoute {
                    chain: "Ethereum".into(),
                    token_address: "0x3333333333333333333333333333333333333333".into(),
                    recipient: Some("recipient".into()),
                    recipient_wallet: None,
                    recipient_setup: false,
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["amount_out"], u128::MAX.to_string());
    assert_eq!(body["bridge_target_chain"], "Ethereum");
    assert_eq!(body["invoice_description"], "Invoice #42");
    assert_eq!(body["gasless"], true);
    assert_eq!(
        client
            .get_stored_swap(&swap.id)
            .unwrap()
            .unwrap()
            .metadata
            .bridge_recipient
            .as_deref(),
        Some("recipient")
    );
}

#[tokio::test]
async fn retries_only_explicit_hash_collisions_with_typescript_index_spacing() {
    use lendaswap_sdk::types::SwapOptions;
    let server = MockServer::start().await;
    Mock::given(path("/swap/bitcoin/arkade"))
        .respond_with({
            let calls = std::sync::atomic::AtomicU32::new(0);
            move |_: &wiremock::Request| {
                if calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
                    ResponseTemplate::new(409).set_body_json(
                        json!({"error":"a swap with this preimage hash exists already"}),
                    )
                } else {
                    ResponseTemplate::new(200)
                        .set_body_json(bitcoin_htlc_response("btc_to_arkade", 11))
                }
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let (client, records) = recording_client(&server);
    let (_, _, _, request) = creation_cases(QuoteAmount::Target(100_000)).remove(0);
    let swap = client
        .create_swap_request(request, SwapOptions::default())
        .await
        .unwrap();
    assert_eq!(
        client.get_stored_swap(&swap.id).unwrap().unwrap().key_index,
        11
    );
    let records = records.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    let failed = &records[0];
    assert_eq!(failed.level, LogLevel::Debug);
    assert_eq!(failed.event.as_deref(), Some("create.retry.attempt_failed"));
    assert_eq!(failed.module.as_deref(), Some("create/retry"));
    assert_eq!(
        failed.operation.as_deref(),
        Some("create.retry_on_hash_collision")
    );
    assert_eq!(
        Value::Object(failed.data.clone()),
        json!({"attempt": 1, "keyIndex": 0})
    );
    assert!(
        failed
            .error
            .as_deref()
            .unwrap()
            .contains("a swap with this preimage hash exists already")
    );
    let requests = server.received_requests().await.unwrap();
    let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let second: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_ne!(first["hash_lock"], second["hash_lock"]);
}

#[tokio::test]
async fn invalid_creation_does_not_allocate_keys_or_send_requests() {
    use lendaswap_sdk::types::EvmAsset;
    use lendaswap_sdk::types::SwapCreation;
    use lendaswap_sdk::types::SwapOptions;
    let server = MockServer::start().await;
    let storage = Arc::new(InMemorySwapStorage::new());
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(storage.clone())
        .build()
        .unwrap();
    let asset = EvmAsset {
        chain_id: 1,
        token: TokenId::UsdcPolygon,
    };
    for request in [
        SwapCreation::EvmToBitcoin {
            source: asset,
            amount: QuoteAmount::Target(100_000),
            target_address: None,
        },
        SwapCreation::BitcoinToArkade {
            sats_receive: 100_000,
            target_address: "ark1invalid".into(),
        },
        SwapCreation::ArkadeToLightning {
            destination: LightningDestination::AddressSendMax {
                address: "test@example.com".into(),
                amount: u128::MAX,
            },
        },
        SwapCreation::LightningToArkade {
            amount: QuoteAmount::Source(0),
            target_address: "invalid".into(),
        },
    ] {
        assert!(
            client
                .create_swap_request(request, SwapOptions::default())
                .await
                .is_err()
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(storage.next_key_index().unwrap(), 0);
}

#[tokio::test]
async fn dex_quote_and_unsigned_calldata_match_api_schema() {
    use lendaswap_sdk::types::*;
    let server = MockServer::start().await;
    let response = json!({"chain":"137","payload":{"kind":"evm","calls":[{"target":"0x1111111111111111111111111111111111111111","data":"0x1234","value":"0"}]},"expected_amount_in":"100000","estimated_amount_out":"200000","estimated_settlement_seconds":0,"router":"uniswap"});
    assert_schema(&response, "DexCalldataResponse");
    for route in ["/dex-calldata/claim", "/dex-calldata/fund"] {
        Mock::given(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(response.clone()))
            .expect(1)
            .mount(&server)
            .await;
    }
    let token = DexToken::Evm {
        chain_id: 137,
        address: "0x1111111111111111111111111111111111111111".into(),
    };
    let request = DexQuoteRequest {
        from: token.clone(),
        to: token.clone(),
        amount: DexAmount::ExactOut(u128::MAX.to_string()),
        slippage_bps: 100,
        cctp_recipient_setup: true,
    };
    assert_schema(&serde_json::to_value(request).unwrap(), "DexQuoteRequest");
    let calldata_request = DexCalldataRequest {
        from: token.clone(),
        to: token,
        amount: DexAmount::ExactIn("100000".into()),
        sender: "0x2222222222222222222222222222222222222222".into(),
        slippage_bps: 100,
        bridge_recipient: None,
    };
    let client = client(&server);
    let response = client
        .get_dex_calldata(calldata_request.clone())
        .await
        .unwrap();
    assert_eq!(response.expected_amount_in, "100000");
    let response = client
        .get_dex_calldata_fund(calldata_request)
        .await
        .unwrap();
    assert_eq!(response.expected_amount_in, "100000");
    let spec: Value = serde_json::from_str(include_str!("../openapi.json")).unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests {
        let route = request.url.path();
        assert!(
            spec["paths"][route]["post"].is_object(),
            "{route} is not a served POST route"
        );
        assert_schema(
            &serde_json::from_slice(&request.body).unwrap(),
            "DexCalldataRequest",
        );
    }
}

const INVOICE_HASH_MISMATCH: &str = "the Lightning invoice does not pay the swap's hash lock";
const ARKADE_DESTINATION: &str = "tark1qqellv77udfmr20tun8dvju5vgudpf9vxe8jwhthrkn26fz96pawqfdy8nk05rsmrf8h94j26905e7n6sng8y059z8ykn2j5xcuw4xt846qj6x";

fn is_invoice_hash_mismatch<T>(result: &Result<T, Error>) -> bool {
    matches!(result, Err(Error::InvalidSwap(message)) if message == INVOICE_HASH_MISMATCH)
}

fn lightning_receive_request(direction: &str) -> lendaswap_sdk::types::SwapCreation {
    use lendaswap_sdk::types::EvmAsset;
    use lendaswap_sdk::types::SwapCreation;
    match direction {
        "lightning_to_arkade" => SwapCreation::LightningToArkade {
            amount: QuoteAmount::Target(100_000),
            target_address: ARKADE_DESTINATION.into(),
        },
        _ => SwapCreation::LightningToEvm {
            target: EvmAsset {
                chain_id: 137,
                token: TokenId::Other("0x1111111111111111111111111111111111111111".into()),
            },
            amount: QuoteAmount::Target(100_000),
            target_address: "0x2222222222222222222222222222222222222222".into(),
        },
    }
}

fn lightning_payout_request(
    direction: &str,
    destination: LightningDestination,
) -> lendaswap_sdk::types::SwapCreation {
    use lendaswap_sdk::types::EvmAsset;
    use lendaswap_sdk::types::SwapCreation;
    match direction {
        "arkade_to_lightning" => SwapCreation::ArkadeToLightning { destination },
        _ => SwapCreation::EvmToLightning {
            source: EvmAsset {
                chain_id: 42161,
                token: TokenId::Usdt0Arbitrum,
            },
            destination,
        },
    }
}

#[test]
fn the_shared_invoices_pay_this_wallets_swap_hashes() {
    let signer = Signer::from_mnemonic(MNEMONIC).unwrap();
    let vectors = lightning_vectors();
    assert_eq!(vectors["mnemonic"], MNEMONIC);
    for (index, name) in [(0, "lnd_hold_index_0"), (7, "lnd_hold_index_7")] {
        assert_eq!(
            hex::encode(signer.derive_swap_params(index).unwrap().hash_lock),
            vectors["invoices"][name]["payment_hash"]
        );
    }
    assert_eq!(
        format!(
            "0x{}",
            hex::encode(signer.derive_swap_params(1).unwrap().hash_lock)
        ),
        vectors["swaps"]["lightning_to_arkade"]["response"]["hash_lock"]
    );
}

#[tokio::test]
async fn lightning_receive_refuses_a_hold_invoice_for_another_hash_before_storing() {
    for (route, direction) in [
        ("lightning/arkade", "lightning_to_arkade"),
        ("lightning/evm", "lightning_to_evm"),
    ] {
        for (paid, refused) in [("lnd_hold_33", true), ("lnd_hold_index_0", false)] {
            let server = MockServer::start().await;
            let mut response = hold_invoice_response(direction, 0);
            response["bolt11_invoice"] = json!(invoice(paid));
            // Once: the refusal is not a hash collision, so no other key is tried.
            Mock::given(method("POST"))
                .and(path(format!("/swap/{route}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(response))
                .expect(1)
                .mount(&server)
                .await;
            let client = client(&server);
            let result = client
                .create_swap_request(lightning_receive_request(direction), Default::default())
                .await;
            if refused {
                assert!(is_invoice_hash_mismatch(&result), "{direction}: {result:?}");
                assert!(client.list_all_swaps().unwrap().is_empty(), "{direction}");
            } else {
                result.unwrap();
                assert_eq!(client.list_all_swaps().unwrap().len(), 1, "{direction}");
            }
        }
    }
}

#[tokio::test]
async fn lightning_payout_refuses_a_hash_lock_its_invoice_does_not_pay() {
    let own = lightning_vectors()["invoices"]["lnd_hold_index_0"]["payment_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    for (route, direction) in [
        ("arkade/lightning", "arkade_to_lightning"),
        ("evm/lightning", "evm_to_lightning"),
    ] {
        // The server resolves an address to an invoice, so only their agreement is
        // checked; a hash lock without 0x or in upper case still agrees.
        for (invoice_name, hash_lock, refused) in [
            ("lnd_hold_22", format!("0x{}", "33".repeat(32)), true),
            ("lnd_hold_index_0", own.to_uppercase(), false),
        ] {
            let server = MockServer::start().await;
            let mut response = created(direction);
            response["client_lightning_invoice"] = json!(invoice(invoice_name));
            response["hash_lock"] = json!(hash_lock);
            Mock::given(method("POST"))
                .and(path(format!("/swap/{route}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(response))
                .expect(1)
                .mount(&server)
                .await;
            let client = client(&server);
            let destination = LightningDestination::Address {
                address: "merchant@example.com".into(),
                sats: 1_000,
            };
            let result = client
                .create_swap_request(
                    lightning_payout_request(direction, destination),
                    Default::default(),
                )
                .await;
            if refused {
                assert!(is_invoice_hash_mismatch(&result), "{direction}: {result:?}");
                assert!(client.list_all_swaps().unwrap().is_empty(), "{direction}");
            } else {
                result.unwrap();
                assert_eq!(client.list_all_swaps().unwrap().len(), 1, "{direction}");
            }
        }
    }
}

#[tokio::test]
async fn lightning_payout_refuses_an_invoice_other_than_the_supplied_one() {
    for (route, direction) in [
        ("arkade/lightning", "arkade_to_lightning"),
        ("evm/lightning", "evm_to_lightning"),
    ] {
        let server = MockServer::start().await;
        // Consistent in itself, but not the invoice the caller asked to pay.
        let mut response = fixture(direction);
        response["client_lightning_invoice"] = json!(invoice("lnd_hold_33"));
        response["hash_lock"] = json!(format!("0x{}", "33".repeat(32)));
        Mock::given(method("POST"))
            .and(path(format!("/swap/{route}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        let client = client(&server);
        let result = client
            .create_swap_request(
                lightning_payout_request(
                    direction,
                    LightningDestination::Invoice(invoice("lnd_hold_22")),
                ),
                Default::default(),
            )
            .await;
        assert!(
            matches!(&result, Err(Error::InvalidSwap(message)) if message.contains("different Lightning invoice than the one supplied")),
            "{direction}: {result:?}"
        );
        assert!(client.list_all_swaps().unwrap().is_empty(), "{direction}");
    }
}

#[tokio::test]
async fn a_malformed_supplied_invoice_is_refused_before_the_request() {
    let mut tampered = invoice("lnd_hold_22");
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'q' { 'p' } else { 'q' });
    for destination in [
        LightningDestination::Invoice("lnbcrt1fixture".into()),
        LightningDestination::InvoiceExpecting {
            invoice: tampered,
            sats: 1_000,
        },
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let storage = Arc::new(InMemorySwapStorage::new());
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(storage.clone())
            .build()
            .unwrap();
        let result = client.create_arkade_to_lightning_swap(destination).await;
        assert!(
            matches!(&result, Err(Error::InvalidSwap(message)) if message.starts_with("invalid BOLT11 invoice")),
            "{result:?}"
        );
        assert_eq!(storage.next_key_index().unwrap(), 0);
    }
}

#[tokio::test]
async fn a_stored_lightning_swap_keeps_its_copy_when_the_server_changes_its_invoice() {
    let substitute = invoice("lnd_hold_33");
    for direction in [
        "lightning_to_arkade",
        "lightning_to_evm",
        "arkade_to_lightning",
        "evm_to_lightning",
        "bitcoin_to_lightning",
    ] {
        let receive = direction.starts_with("lightning_");
        let stored = if receive {
            hold_invoice_response(direction, 0)
        } else {
            fixture(direction)
        };
        let id = stored["id"].as_str().unwrap().to_owned();
        let mut substituted = stored.clone();
        if receive {
            substituted["bolt11_invoice"] = json!(substitute);
        } else {
            // Consistent in itself, but not the stored swap's invoice.
            substituted["client_lightning_invoice"] = json!(substitute);
            substituted["hash_lock"] = json!(format!("0x{}", "33".repeat(32)));
        }
        // A payout's lock must stay under the stored invoice's hash, even with that invoice.
        let mut relocked = stored.clone();
        if !receive {
            relocked["hash_lock"] = json!(format!("0x{}", "33".repeat(32)));
        }
        let mut redirected = fixture("arkade_to_evm");
        redirected["id"] = json!(id);
        for (served, refused) in [
            (substituted, true),
            (relocked, !receive),
            (redirected, true),
            (stored.clone(), false),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(format!("/swap/{id}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(served))
                .mount(&server)
                .await;
            let storage = Arc::new(InMemorySwapStorage::new());
            storage
                .store_swap(
                    lendaswap_sdk::StoredSwap::new(
                        0,
                        serde_json::from_value(stored.clone()).unwrap(),
                        Default::default(),
                    )
                    .unwrap(),
                )
                .unwrap();
            let client = Client::builder()
                .base_url(server.uri())
                .mnemonic(MNEMONIC)
                .storage(storage.clone())
                .build()
                .unwrap();
            let results = [
                client.get_swap(&id).await.map(|_| ()),
                client.get_swap_details(&id).await.map(|_| ()),
                client.refresh_swap(&id).await.map(|_| ()),
            ];
            for result in results {
                assert_eq!(
                    matches!(result, Err(Error::InvalidSwap(_))),
                    refused,
                    "{direction}: {result:?}"
                );
            }
            assert_eq!(
                storage.get_swap(&id).unwrap().unwrap().response,
                serde_json::from_value(stored.clone()).unwrap(),
                "{direction}"
            );
        }
    }
}

/// A server that answers for one swap with another's copy, so that the checks of the
/// swap asked for would not apply, is refused whatever the client stores for either.
#[tokio::test]
async fn a_response_for_another_swap_is_refused() {
    const ASKED: &str = "00000000-0000-0000-0000-00000000000a";
    let stored = hold_invoice_response("lightning_to_arkade", 0);
    let mut answered = stored.clone();
    answered["id"] = json!("00000000-0000-0000-0000-00000000beef");
    answered["bolt11_invoice"] = json!(invoice("lnd_hold_33"));
    let mut record = stored.clone();
    record["id"] = json!(ASKED);
    let record: GetSwapResponse = serde_json::from_value(record).unwrap();
    for local in ["record", "index", "nothing"] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/swap/{ASKED}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(&answered))
            .mount(&server)
            .await;
        let storage = Arc::new(InMemorySwapStorage::new());
        match local {
            "record" => storage
                .store_swap(
                    lendaswap_sdk::StoredSwap::new(0, record.clone(), Default::default()).unwrap(),
                )
                .unwrap(),
            "index" => storage.put_swap_key_index(ASKED, 0).unwrap(),
            _ => {}
        }
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(storage.clone())
            .build()
            .unwrap();
        let mut results = vec![
            client.get_swap(ASKED).await.map(|_| ()),
            client.get_swap_details(ASKED).await.map(|_| ()),
            client
                .wait_for_swap_status(ASKED, &[SwapStatus::Pending], Duration::from_secs(5))
                .await
                .map(|_| ()),
        ];
        if local != "nothing" {
            results.push(client.refresh_swap(ASKED).await.map(|_| ()));
        }
        for result in results {
            assert!(
                matches!(&result, Err(Error::InvalidSwap(message)) if message.contains("returned swap")),
                "{local}: {result:?}"
            );
        }
        let kept = client
            .get_stored_swap(ASKED)
            .unwrap()
            .map(|swap| swap.response);
        assert_eq!(kept, (local == "record").then(|| record.clone()), "{local}");
    }
}

/// The server answers in the lower-case spelling the store keys swaps by, so a swap
/// asked for in upper case is the same swap and keeps its checks.
#[tokio::test]
async fn a_swap_asked_for_in_upper_case_keeps_its_checks() {
    const STORED: &str = "00000000-0000-0000-0000-00000000000a";
    let asked = STORED.to_uppercase();
    let mut stored = hold_invoice_response("lightning_to_arkade", 0);
    stored["id"] = json!(STORED);
    let mut substituted = stored.clone();
    substituted["bolt11_invoice"] = json!(invoice("lnd_hold_33"));
    for local in ["record", "index"] {
        for (served, refused) in [(&substituted, true), (&stored, false)] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(format!("/swap/{asked}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(served))
                .mount(&server)
                .await;
            let storage = Arc::new(InMemorySwapStorage::new());
            if local == "record" {
                storage
                    .store_swap(
                        lendaswap_sdk::StoredSwap::new(
                            0,
                            serde_json::from_value(stored.clone()).unwrap(),
                            Default::default(),
                        )
                        .unwrap(),
                    )
                    .unwrap();
            } else {
                storage.put_swap_key_index(STORED, 0).unwrap();
            }
            let client = Client::builder()
                .base_url(server.uri())
                .mnemonic(MNEMONIC)
                .storage(storage)
                .build()
                .unwrap();
            for result in [
                client.get_swap(&asked).await.map(|_| ()),
                client.get_swap_details(&asked).await.map(|_| ()),
            ] {
                if refused {
                    assert!(is_invoice_hash_mismatch(&result), "{local}: {result:?}");
                } else {
                    assert!(result.is_ok(), "{local}: {result:?}");
                }
            }
        }
    }
}

/// The store matches ids exactly, so a response that spells a stored swap's id in
/// upper case would escape its checks, and refresh would store it a second time.
#[tokio::test]
async fn a_swap_served_in_upper_case_is_refused() {
    const STORED: &str = "00000000-0000-0000-0000-00000000000a";
    let mut stored = hold_invoice_response("lightning_to_arkade", 0);
    stored["id"] = json!(STORED);
    let mut relabelled = stored.clone();
    relabelled["id"] = json!(STORED.to_uppercase());
    let mut substituted = relabelled.clone();
    substituted["bolt11_invoice"] = json!(invoice("lnd_hold_33"));
    for local in ["record", "index"] {
        for served in [&relabelled, &substituted] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(format!("/swap/{STORED}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(served))
                .mount(&server)
                .await;
            let storage = Arc::new(InMemorySwapStorage::new());
            if local == "record" {
                storage
                    .store_swap(
                        lendaswap_sdk::StoredSwap::new(
                            0,
                            serde_json::from_value(stored.clone()).unwrap(),
                            Default::default(),
                        )
                        .unwrap(),
                    )
                    .unwrap();
            } else {
                storage.put_swap_key_index(STORED, 0).unwrap();
            }
            let client = Client::builder()
                .base_url(server.uri())
                .mnemonic(MNEMONIC)
                .storage(storage.clone())
                .build()
                .unwrap();
            for result in [
                client.get_swap(STORED).await.map(|_| ()),
                client.get_swap_details(STORED).await.map(|_| ()),
                client.refresh_swap(STORED).await.map(|_| ()),
            ] {
                assert!(
                    matches!(&result, Err(Error::InvalidSwap(message)) if message.contains("returned swap")),
                    "{local}: {result:?}"
                );
            }
            let ids: Vec<_> = client
                .list_all_swaps()
                .unwrap()
                .into_iter()
                .map(|swap| swap.response.id().to_owned())
                .collect();
            let expected: &[&str] = if local == "record" { &[STORED] } else { &[] };
            assert_eq!(ids, expected, "{local}");
            assert_eq!(
                storage.get_swap_key_index(&STORED.to_uppercase()).unwrap(),
                None,
                "{local}"
            );
        }
    }
}

/// A swap created under an upper-case id would be stored under a spelling that
/// fetching it never looks up.
#[tokio::test]
async fn creation_refuses_a_swap_id_with_upper_case_letters() {
    const ID: &str = "3130e20f-92b3-4825-b54f-fac2872dba4f";
    for (id, refused) in [(ID.to_uppercase(), true), (ID.to_owned(), false)] {
        let server = MockServer::start().await;
        let mut response = hold_invoice_response("lightning_to_arkade", 0);
        response["id"] = json!(id);
        // Once: the refusal is not a hash collision, so no other key is tried.
        Mock::given(method("POST"))
            .and(path("/swap/lightning/arkade"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;
        let client = client(&server);
        let result = client
            .create_swap_request(
                lightning_receive_request("lightning_to_arkade"),
                Default::default(),
            )
            .await;
        if refused {
            assert!(
                matches!(&result, Err(Error::InvalidSwap(message)) if message.contains("upper-case")),
                "{result:?}"
            );
            assert!(client.list_all_swaps().unwrap().is_empty());
        } else {
            result.unwrap();
            assert_eq!(client.list_all_swaps().unwrap().len(), 1);
        }
    }
}

/// A server that relabels a stored swap as a Lightning one could otherwise hand over
/// an invoice no hash of the client's backs.
#[tokio::test]
async fn a_stored_swap_keeps_its_direction() {
    let stored = fixture("arkade_to_evm");
    let id = stored["id"].as_str().unwrap().to_owned();
    let mut relabelled = hold_invoice_response("lightning_to_arkade", 0);
    relabelled["id"] = json!(id);
    relabelled["bolt11_invoice"] = json!(invoice("lnd_hold_33"));
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/swap/{id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(relabelled))
        .mount(&server)
        .await;
    let storage = Arc::new(InMemorySwapStorage::new());
    let stored: GetSwapResponse = serde_json::from_value(stored).unwrap();
    storage
        .store_swap(lendaswap_sdk::StoredSwap::new(0, stored.clone(), Default::default()).unwrap())
        .unwrap();
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(storage.clone())
        .build()
        .unwrap();
    for result in [
        client.get_swap(&id).await.map(|_| ()),
        client.get_swap_details(&id).await.map(|_| ()),
        client.refresh_swap(&id).await.map(|_| ()),
    ] {
        assert!(
            matches!(&result, Err(Error::InvalidSwap(message)) if message.contains("differ from the stored swap")),
            "{result:?}"
        );
    }
    assert_eq!(storage.get_swap(&id).unwrap().unwrap().response, stored);
}

/// A legacy database upgraded on open, or a store that keeps only key indices after
/// a restart, holds no record to compare with, only the key index.
#[tokio::test]
async fn an_index_only_lightning_swap_gets_the_invoice_checks_of_recovery() {
    for direction in [
        "lightning_to_arkade",
        "lightning_to_evm",
        "arkade_to_lightning",
        "evm_to_lightning",
        "bitcoin_to_lightning",
    ] {
        let receive = direction.starts_with("lightning_");
        let valid = if receive {
            hold_invoice_response(direction, 0)
        } else {
            fixture(direction)
        };
        let id = valid["id"].as_str().unwrap().to_owned();
        let mut invalid = valid.clone();
        if receive {
            invalid["bolt11_invoice"] = json!(invoice("lnd_hold_33"));
        } else {
            invalid["hash_lock"] = json!(format!("0x{}", "33".repeat(32)));
        }
        for (served, refused) in [(invalid, true), (valid, false)] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(format!("/swap/{id}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(&served))
                .mount(&server)
                .await;
            let storage = Arc::new(InMemorySwapStorage::new());
            storage.put_swap_key_index(&id, 0).unwrap();
            let client = Client::builder()
                .base_url(server.uri())
                .mnemonic(MNEMONIC)
                .storage(storage.clone())
                .build()
                .unwrap();
            let results = [
                client.get_swap(&id).await.map(|_| ()),
                client.get_swap_details(&id).await.map(|_| ()),
                client.refresh_swap(&id).await.map(|_| ()),
            ];
            for result in results {
                assert_eq!(
                    is_invoice_hash_mismatch(&result),
                    refused,
                    "{direction}: {result:?}"
                );
            }
            let stored = client.get_stored_swap(&id).unwrap();
            if refused {
                assert!(stored.is_none(), "{direction}");
            } else {
                assert_eq!(
                    stored.unwrap().response,
                    serde_json::from_value(served).unwrap(),
                    "{direction}"
                );
            }
        }
    }
}

/// A real swap response from the shared vectors as a recovery entry at `index`.
fn recovered_vector(direction: &str, index: u32) -> Value {
    let mut swap = lightning_vectors()["swaps"][direction]["response"].clone();
    swap["direction"] = json!(direction);
    swap["index"] = json!(index);
    swap
}

#[tokio::test]
async fn recovery_refuses_lightning_swaps_whose_invoice_does_not_pay_their_hash_lock() {
    let mut receive = recovered_vector("lightning_to_arkade", 1);
    receive["bolt11_invoice"] = json!(invoice("lnd_hold_33"));
    let mut payout = recovered_vector("arkade_to_lightning", 1);
    payout["hash_lock"] = json!(format!("0x{}", "33".repeat(32)));
    for (swap, refused) in [
        (recovered_vector("lightning_to_arkade", 1), false),
        (recovered_vector("arkade_to_lightning", 1), false),
        (receive, true),
        (payout, true),
    ] {
        let server = MockServer::start().await;
        let id = swap["id"].as_str().unwrap().to_owned();
        Mock::given(path("/swap/recover"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "swaps":[swap],"highest_index":1,"next_index":2,"scanned_until":10,"complete":true
            })))
            .mount(&server)
            .await;
        let storage = Arc::new(InMemorySwapStorage::new());
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .storage(storage.clone())
            .build()
            .unwrap();
        let result = client.recover_swaps(None).await;
        if refused {
            assert!(is_invoice_hash_mismatch(&result), "{id}: {result:?}");
            assert_eq!(storage.get_swap_key_index(&id).unwrap(), None);
            assert_eq!(storage.next_key_index().unwrap(), 0);
        } else {
            result.unwrap();
            assert_eq!(storage.get_swap_key_index(&id).unwrap(), Some(1));
        }
    }
}

#[tokio::test]
async fn recovery_keeps_a_stored_lightning_payout_the_server_would_replace() {
    let stored = fixture("arkade_to_lightning");
    let own_key = Signer::from_mnemonic(MNEMONIC)
        .unwrap()
        .derive_swap_params(0)
        .unwrap()
        .public_key;
    // It passes every check a swap new to this client gets.
    let mut substituted = stored.clone();
    substituted["client_lightning_invoice"] = json!(invoice("lnd_hold_33"));
    substituted["hash_lock"] = json!(format!("0x{}", "33".repeat(32)));
    substituted["sender_pk"] = json!(hex::encode(own_key));
    substituted["index"] = json!(0);
    let server = MockServer::start().await;
    Mock::given(path("/swap/recover"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "swaps":[substituted],"highest_index":0,"next_index":1,"scanned_until":10,"complete":true
        })))
        .mount(&server)
        .await;
    let storage = Arc::new(InMemorySwapStorage::new());
    storage
        .store_swap(
            lendaswap_sdk::StoredSwap::new(
                0,
                serde_json::from_value(stored.clone()).unwrap(),
                Default::default(),
            )
            .unwrap(),
        )
        .unwrap();
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(storage.clone())
        .build()
        .unwrap();
    let result = client.recover_swaps(None).await;
    assert!(is_invoice_hash_mismatch(&result), "{result:?}");
    let id = stored["id"].as_str().unwrap();
    assert_eq!(
        storage.get_swap(id).unwrap().unwrap().response,
        serde_json::from_value(stored).unwrap()
    );
}
