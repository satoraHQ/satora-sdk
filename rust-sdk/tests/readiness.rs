//! Current API compatibility and restart regressions, without external services.
use lendaswap_sdk::Client;
use lendaswap_sdk::Error;
use lendaswap_sdk::InMemorySwapStorage;
use lendaswap_sdk::Signer;
use lendaswap_sdk::SwapStorage;
use lendaswap_sdk::types::Address;
use lendaswap_sdk::types::GetSwapResponse;
use lendaswap_sdk::types::LightningDestination;
use lendaswap_sdk::types::QuoteAmount;
use lendaswap_sdk::types::SwapStatus;
use lendaswap_sdk::types::TokenId;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
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

fn client(server: &MockServer) -> Client {
    Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .build()
        .unwrap()
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

#[tokio::test]
async fn lightning_receive_uses_current_amount_fields_script_version_and_fees() {
    for amount in [QuoteAmount::Source(100_000), QuoteAmount::Target(100_000)] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/swap/lightning/arkade"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture("lightning_to_arkade")))
            .expect(1)
            .mount(&server)
            .await;
        let swap = client(&server)
            .create_lightning_to_arkade_swap_with_options(
                amount.clone(),
                Address::Arkade("ark1fixture".into()),
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
    let err = client(&server)
        .create_arkade_to_lightning_swap(LightningDestination::Invoice("lnbcrt1fixture".into()))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Api { status: 409, .. }));
}

#[tokio::test]
async fn get_swap_sends_compatibility_headers_for_every_direction() {
    let server = MockServer::start().await;
    let spec: Value = serde_json::from_str(include_str!("../openapi.json")).unwrap();
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
        Mock::given(method("GET"))
            .and(path(format!("/swap/{direction}")))
            .and(header(
                "x-satora-server-version",
                spec["info"]["version"].as_str().unwrap(),
            ))
            .and(header(
                "X-Lendaswap-Client",
                concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture(direction)))
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
    Mock::given(method("GET"))
        .and(path("/swap/pending"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("evm_to_arkade")))
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
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage(storage.clone())
        .build()
        .unwrap();
    let recovery = client.recover_all_swaps(None).await.unwrap();
    assert_eq!(recovery.swaps.len(), 2);
    assert!(recovery.complete);
    assert_eq!(storage.get_swap_key_index("swap-3").unwrap(), Some(3));
    assert_eq!(storage.get_swap_key_index("swap-12").unwrap(), Some(12));
    assert_eq!(storage.next_key_index().unwrap(), 50);
}

#[tokio::test]
async fn recovery_rejects_a_wrong_wallet_before_mutating_storage() {
    let server = MockServer::start().await;
    let mut swap = recovered(3);
    swap["hash_lock"] = json!("00".repeat(32));
    Mock::given(path("/swap/recover"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "swaps":[swap],"highest_index":3,"next_index":4,"scanned_until":10,"complete":true
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
    assert!(matches!(
        client.recover_swaps(None).await,
        Err(Error::InvalidSwap(_))
    ));
    assert_eq!(storage.get_swap_key_index("swap-3").unwrap(), None);
    assert_eq!(storage.next_key_index().unwrap(), 0);
}

#[tokio::test]
async fn persistent_client_reopens_the_same_wallet_namespace() {
    let server = MockServer::start().await;
    Mock::given(path("/swap/lightning/arkade"))
        .respond_with({
            let next_id = std::sync::atomic::AtomicU32::new(0);
            move |_: &wiremock::Request| {
                let mut response = fixture("lightning_to_arkade");
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
            .create_lightning_to_arkade_swap(100_000, Address::Arkade("ark1fixture".into()))
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
                Address::Arkade("ark1fixture".into()),
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
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("evm_to_lightning")))
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
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("lightning_to_arkade")))
        .expect(1)
        .mount(&server)
        .await;
    client(&server)
        .create_swap(
            lendaswap_sdk::types::KnownChain::Lightning,
            TokenId::Btc,
            TokenId::Btc,
            QuoteAmount::Source(100_000),
            Some(Address::Arkade("ark1fixture".into())),
            Some(15),
        )
        .await
        .unwrap();
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
