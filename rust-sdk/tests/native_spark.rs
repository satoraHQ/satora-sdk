//! Native Spark HTTP creation and persistence, with a mock swap server.
use lendaswap_sdk::Client;
use lendaswap_sdk::SwapFunding;
use lendaswap_sdk::types::EvmAsset;
use lendaswap_sdk::types::QuoteAmount;
use lendaswap_sdk::types::SwapCreation;
use lendaswap_sdk::types::SwapOptions;
use lendaswap_sdk::types::TokenId;
use serde_json::Value;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn response(request: &Request, bad_hash: bool) -> Value {
    let body: Value = request.body_json().unwrap();
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/swap-responses.json")).unwrap();
    let mut data = fixtures["lightning_to_evm"].clone();
    let address = bitcoin::bech32::encode::<bitcoin::bech32::Bech32m>(
        bitcoin::bech32::Hrp::parse("sparkrt").unwrap(),
        &[2; 33],
    )
    .unwrap();
    data["direction"] = json!("spark_to_evm");
    data["source_token"]["chain"] = json!("Spark");
    data["hash_lock"] = if bad_hash {
        json!(format!("0x{}", "00".repeat(32)))
    } else {
        body["hash_lock"].clone()
    };
    data["client_evm_address"] = body["claiming_address"].clone();
    data["target_evm_address"] = body["target_address"].clone();
    data["source_amount"] = json!("20000");
    data["funding"] =
        json!({ "address": address, "expiry_duration_secs": 3600, "payment_id": null });
    data["funding_expires_at"] = json!(2_000_000_000);
    data.as_object_mut().unwrap().remove("bolt11_invoice");
    data.as_object_mut().unwrap().remove("invoice_expires_at");
    data
}

fn request() -> SwapCreation {
    SwapCreation::SparkToEvm {
        target: EvmAsset {
            chain_id: 42161,
            token: TokenId::UsdcArbitrum,
        },
        amount: QuoteAmount::Source(20000),
        target_address: "0x000000000000000000000000000000000000dEaD".into(),
    }
}

#[tokio::test]
async fn native_spark_creation_survives_restart_without_an_invoice() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/swap/spark/evm"))
        .respond_with(|r: &Request| ResponseTemplate::new(200).set_body_json(response(r, false)))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("native.db");
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage_path(&db)
        .build()
        .unwrap();
    let swap = client
        .create_swap_request(request(), SwapOptions::default())
        .await
        .unwrap();
    assert!(matches!(
        swap.funding,
        SwapFunding::SparkHtlc {
            expiry_duration_secs: 3600,
            ..
        }
    ));
    drop(client);
    let reopened = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .storage_path(&db)
        .build()
        .unwrap();
    let stored = reopened.get_stored_swap(&swap.id).unwrap().unwrap();
    assert!(matches!(
        stored.response,
        lendaswap_sdk::types::GetSwapResponse::SparkToEvm(_)
    ));
}

#[tokio::test]
async fn native_spark_rejects_a_hash_not_owned_by_the_client() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/swap/spark/evm"))
        .respond_with(|r: &Request| ResponseTemplate::new(200).set_body_json(response(r, true)))
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(MNEMONIC)
        .build()
        .unwrap();
    assert!(
        client
            .create_swap_request(request(), SwapOptions::default())
            .await
            .is_err()
    );
    assert!(client.list_all_swaps().unwrap().is_empty());
}

#[tokio::test]
async fn native_spark_rejects_unexpected_or_malformed_claim_addresses_before_persistence() {
    for override_address in [None, Some("0x1111111111111111111111111111111111111111")] {
        for returned in [
            "0x2222222222222222222222222222222222222222",
            "not-an-address",
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/swap/spark/evm"))
                .respond_with(move |r: &Request| {
                    let mut data = response(r, false);
                    data["client_evm_address"] = json!(returned);
                    ResponseTemplate::new(200).set_body_json(data)
                })
                .mount(&server)
                .await;
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("native.db");
            let client = Client::builder()
                .base_url(server.uri())
                .mnemonic(MNEMONIC)
                .storage_path(&db)
                .build()
                .unwrap();
            let result = client
                .create_swap_request(
                    request(),
                    SwapOptions {
                        evm_address: override_address.map(str::to_owned),
                        ..Default::default()
                    },
                )
                .await;
            assert!(
                matches!(result, Err(lendaswap_sdk::Error::InvalidSwap(_))),
                "unexpected claim address must not expose funding: {result:?}"
            );
            assert!(client.list_all_swaps().unwrap().is_empty());
            drop(client);
            let reopened = Client::builder()
                .base_url(server.uri())
                .mnemonic(MNEMONIC)
                .storage_path(&db)
                .build()
                .unwrap();
            assert!(reopened.list_all_swaps().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn native_spark_accepts_requested_claim_addresses_independent_of_hex_case() {
    for override_address in [None, Some("0x000000000000000000000000000000000000dEaD")] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/swap/spark/evm"))
            .respond_with(|r: &Request| {
                let mut data = response(r, false);
                let sent = data["client_evm_address"].as_str().unwrap();
                data["client_evm_address"] = json!(format!("0x{}", sent[2..].to_ascii_uppercase()));
                ResponseTemplate::new(200).set_body_json(data)
            })
            .mount(&server)
            .await;
        let client = Client::builder()
            .base_url(server.uri())
            .mnemonic(MNEMONIC)
            .build()
            .unwrap();
        let swap = client
            .create_swap_request(
                request(),
                SwapOptions {
                    evm_address: override_address.map(str::to_owned),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let requests = server.received_requests().await.unwrap();
        let sent: Value = requests[0].body_json().unwrap();
        if let Some(expected) = override_address {
            assert_eq!(sent["claiming_address"], expected);
        }
        let stored = client.get_stored_swap(&swap.id).unwrap().unwrap();
        let lendaswap_sdk::types::GetSwapResponse::SparkToEvm(native) = stored.response else {
            panic!("expected native swap")
        };
        assert_eq!(
            native.client_evm_address.to_ascii_lowercase(),
            sent["claiming_address"]
                .as_str()
                .unwrap()
                .to_ascii_lowercase()
        );
    }
}
