use super::*;
use crate::Error;
use crate::control::PollOptions;
use alloy::primitives::Address;
use alloy::primitives::B256;
use alloy::primitives::U256;
use cctp::*;
use serde_json::json;
use std::time::Duration;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

pub(crate) fn burn() -> CctpBurn {
    CctpBurn {
        source_chain_id: 8453,
        source_domain: 6,
        destination_domain: 3,
        source_messenger: TOKEN_MESSENGER,
        destination_messenger: TOKEN_MESSENGER.into_word(),
        source_transmitter: MESSAGE_TRANSMITTER,
        burn_token: CctpChain::for_chain(8453).unwrap().usdc,
        sender: Address::repeat_byte(1),
        mint_recipient: Address::repeat_byte(2).into_word(),
        destination_caller: B256::ZERO,
        amount: U256::from(10_000_000),
        max_fee: U256::from(1000),
        min_finality: 1000,
        hook: forwarding_hook(None),
    }
}
pub(crate) fn message(b: &CctpBurn) -> Vec<u8> {
    let mut result = Vec::new();
    result.extend(1u32.to_be_bytes());
    result.extend(b.source_domain.to_be_bytes());
    result.extend(b.destination_domain.to_be_bytes());
    result.extend([7; 32]);
    result.extend(b.source_messenger.into_word());
    result.extend(b.destination_messenger);
    result.extend(b.destination_caller);
    result.extend(b.min_finality.to_be_bytes());
    result.extend(1000u32.to_be_bytes());
    result.extend(1u32.to_be_bytes());
    result.extend(b.burn_token.into_word());
    result.extend(b.mint_recipient);
    result.extend(b.amount.to_be_bytes::<32>());
    result.extend(b.sender.into_word());
    result.extend(b.max_fee.to_be_bytes::<32>());
    result.extend(U256::from(100).to_be_bytes::<32>());
    result.extend(U256::from(123).to_be_bytes::<32>());
    result.extend(b.hook.iter());
    result
}
fn attested(b: &CctpBurn) -> serde_json::Value {
    json!({"message":format!("0x{}",hex::encode(message(b))),"attestation":"0x1234","status":"complete"})
}
#[test]
fn cctp_layout_rejects_changed_immutable_fields_and_excess_fees() {
    let burn = burn();
    let original = message(&burn);
    assert_eq!(
        burn.validate_message(&original).unwrap().minted_amount(),
        U256::from(9_999_900)
    );
    for index in [0, 4, 8, 44, 76, 108, 140, 148, 152, 184, 216, 248, 280, 376] {
        let mut mutated = original.clone();
        mutated[index] ^= 1;
        assert!(burn.validate_message(&mutated).is_err(), "offset {index}");
    }
    let mut mutable = original.clone();
    mutable[12] ^= 1;
    mutable[344] ^= 1;
    assert!(burn.validate_message(&mutable).is_ok()); // Circle assigns these after the burn.
    let mut too_expensive = original.clone();
    too_expensive[312..344].copy_from_slice(&U256::from(1001).to_be_bytes::<32>());
    assert!(burn.validate_message(&too_expensive).is_err());
    for length in [0, 12, 147, 375] {
        assert!(CctpMessage::decode(&original[..length]).is_err());
    }
}
#[test]
fn registries_fees_and_solana_hooks_match_the_reference() {
    assert_eq!(CHAINS.len(), 15);
    assert_eq!(usdt0::CHAINS.len(), 22);
    assert_eq!(CctpChain::for_chain(50).unwrap().finality(), 2000);
    assert!(CctpChain::for_chain(8453).unwrap().requires_bridge());
    assert!(!CctpChain::for_chain(42161).unwrap().requires_bridge());
    let hook = forwarding_hook(Some([42; 32]));
    assert_eq!(hook.len(), 65);
    assert_eq!(&hook[28..33], &[0, 0, 0, 33, 1]);
    assert_eq!(&hook[33..], &[42; 32]);
    let fee: CctpFee = serde_json::from_value(
        json!({"finalityThreshold":1000,"minimumFee":1.3,"forwardFee":{"high":200000}}),
    )
    .unwrap();
    assert_eq!(
        fee.max_fee(U256::from(100_000_000), false).unwrap(),
        U256::from(15600)
    );
    assert_eq!(
        fee.max_fee(U256::from(100_000_000), true).unwrap(),
        U256::from(215600)
    );
    assert!(fee.max_fee(U256::MAX, false).is_err());
    assert_ne!(
        usdt0::ARBITRUM_OFT,
        usdt0::CHAINS
            .iter()
            .find(|c| c.name == "Arbitrum")
            .unwrap()
            .token
    );
}
#[tokio::test]
async fn attestation_selection_timeout_and_cancellation() {
    let server = MockServer::start().await;
    let api = BridgeApi::new(&server.uri(), &server.uri()).unwrap();
    let expected = burn();
    let mut other = expected.clone();
    other.mint_recipient = B256::repeat_byte(9);
    Mock::given(path("/v2/messages/6"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"messages":[attested(&other),attested(&expected)]})),
        )
        .mount(&server)
        .await;
    let found = api
        .cctp_attestation(B256::repeat_byte(4), &expected)
        .await
        .unwrap()
        .unwrap();
    found.receive_calldata(&expected).unwrap();
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(10)))
        .mount(&server)
        .await;
    let options = PollOptions {
        timeout: Duration::from_millis(40),
        interval: Duration::from_millis(1),
        ..Default::default()
    };
    assert!(matches!(
        api.wait_cctp_attestation(B256::repeat_byte(4), &expected, &options)
            .await,
        Err(Error::Timeout(_))
    ));
    options.cancellation.cancel();
    assert!(matches!(
        api.wait_cctp_attestation(B256::repeat_byte(4), &expected, &options)
            .await,
        Err(Error::Cancelled)
    ));
}
fn lz_entry(
    guid: B256,
    source: impl serde::Serialize,
    status: &str,
    destination_eid: u32,
    destination: serde_json::Value,
) -> serde_json::Value {
    json!({"guid":guid,"status":{"name":status,"message":"from Scan"},"pathway":{"srcEid":30110,"dstEid":destination_eid,"sender":{"chain":"arbitrum"},"receiver":{"chain":"optimism"}},"source":{"tx":{"txHash":source}},"destination":destination})
}
async fn lz_respond(server: &MockServer, body: serde_json::Value) {
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}
fn quick(timeout: u64) -> PollOptions {
    PollOptions {
        timeout: Duration::from_millis(timeout),
        interval: Duration::from_millis(1),
        ..Default::default()
    }
}
#[tokio::test]
async fn layerzero_query_selects_by_guid_and_checks_identity() {
    let server = MockServer::start().await;
    let api = BridgeApi::new(&server.uri(), &server.uri()).unwrap();
    let query = usdt0::MessageQuery {
        source_txid: B256::repeat_byte(3),
        source_eid: 30110,
        destination_eid: 30111,
        guid: B256::repeat_byte(4),
    };
    let tx = |hash: u8| json!({"tx":{"txHash":B256::repeat_byte(hash)}});
    lz_respond(
        &server,
        json!({"data":[lz_entry(B256::repeat_byte(9),query.source_txid,"DELIVERED",30111,tx(7)),lz_entry(query.guid,query.source_txid,"DELIVERED",30111,tx(8))]}),
    )
    .await;
    assert_eq!(
        api.usdt0_status(&query)
            .await
            .unwrap()
            .unwrap()
            .destination_txid,
        Some(B256::repeat_byte(8))
    );
    lz_respond(
        &server,
        json!({"data":[lz_entry(query.guid,query.source_txid,"INFLIGHT",30101,json!({}))]}),
    )
    .await;
    assert!(
        matches!(api.usdt0_status(&query).await, Err(Error::InvalidSwap(m)) if m.contains("identity"))
    );
    lz_respond(
        &server,
        json!({"data":[lz_entry(query.guid,query.source_txid,"BLOCKED",30111,json!({}))]}),
    )
    .await;
    assert!(matches!(
        api.wait_usdt0(&query, &quick(2000)).await,
        Err(Error::InvalidSwap(_))
    ));
}
#[tokio::test]
async fn layerzero_tx_lookup_returns_chain_names() {
    let server = MockServer::start().await;
    let api = BridgeApi::new(&server.uri(), &format!("{}/scan/", server.uri())).unwrap();
    let txid = B256::repeat_byte(3);
    Mock::given(path(format!("/scan/v1/messages/tx/{txid}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data":[lz_entry(B256::repeat_byte(4),txid,"DELIVERED",30111,json!({"status":"SUCCEEDED"}))]}),
        ))
        .mount(&server)
        .await;
    let delivery = api
        .usdt0_status_by_tx(&usdt0::MessageLookup::tx(txid))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        delivery,
        usdt0::Delivery {
            guid: B256::repeat_byte(4),
            status: usdt0::DeliveryStatus::Delivered,
            source_txid: txid,
            destination_txid: None,
            source_eid: 30110,
            destination_eid: 30111,
            source_chain: Some("arbitrum".into()),
            destination_chain: Some("optimism".into()),
        }
    );
    assert_eq!(
        api.usdt0_status_by_tx(&usdt0::MessageLookup::tx(B256::repeat_byte(5)))
            .await
            .unwrap(),
        None
    );
}
#[tokio::test]
async fn layerzero_tx_lookup_never_guesses() {
    let server = MockServer::start().await;
    let api = BridgeApi::new(&server.uri(), &server.uri()).unwrap();
    let txid = B256::repeat_byte(3);
    let (first, second) = (B256::repeat_byte(4), B256::repeat_byte(5));
    // Scan also lists messages this transaction delivered, from any source chain.
    let from_solana = lz_entry(
        B256::repeat_byte(6),
        "29zT7SLdFZkKsolana",
        "DELIVERED",
        30110,
        json!({"tx":{"txHash":txid}}),
    );
    let foreign = B256::repeat_byte(7);
    let mut from_ethereum = lz_entry(
        foreign,
        B256::repeat_byte(9),
        "DELIVERED",
        30110,
        json!({"tx":{"txHash":txid}}),
    );
    from_ethereum["pathway"]["srcEid"] = json!(30101);
    lz_respond(
        &server,
        json!({"data":[from_solana,from_ethereum.clone(),lz_entry(first,txid,"INFLIGHT",30111,json!({})),lz_entry(second,txid,"INFLIGHT",30101,json!({}))]}),
    )
    .await;
    let lookup = usdt0::MessageLookup::tx(txid);
    assert!(
        matches!(api.usdt0_status_by_tx(&lookup).await, Err(Error::InvalidSwap(m)) if m.starts_with("2 LayerZero messages"))
    );
    let narrowed = usdt0::MessageLookup {
        destination_eid: Some(30101),
        ..lookup.clone()
    };
    assert_eq!(
        api.usdt0_status_by_tx(&narrowed)
            .await
            .unwrap()
            .unwrap()
            .guid,
        second
    );
    let by_guid = usdt0::MessageLookup {
        guid: Some(second),
        ..lookup.clone()
    };
    assert_eq!(
        api.usdt0_status_by_tx(&by_guid)
            .await
            .unwrap()
            .unwrap()
            .destination_eid,
        30101
    );
    let wrong_source = usdt0::MessageLookup {
        source_eid: Some(30101),
        ..lookup.clone()
    };
    assert_eq!(api.usdt0_status_by_tx(&wrong_source).await.unwrap(), None);
    // Before Scan indexes our own message, another transfer's delivery must not stand in for it.
    lz_respond(&server, json!({"data":[from_ethereum]})).await;
    assert_eq!(api.usdt0_status_by_tx(&lookup).await.unwrap(), None);
    let foreign_guid = usdt0::MessageLookup {
        guid: Some(foreign),
        ..lookup
    };
    assert!(
        matches!(api.usdt0_status_by_tx(&foreign_guid).await, Err(Error::InvalidSwap(m)) if m.contains("identity"))
    );
}
#[tokio::test]
async fn layerzero_wait_stops_when_the_message_cannot_be_identified() {
    let server = MockServer::start().await;
    let api = BridgeApi::new(&server.uri(), &server.uri()).unwrap();
    let txid = B256::repeat_byte(3);
    let (first, second) = (B256::repeat_byte(4), B256::repeat_byte(5));
    let lookup = usdt0::MessageLookup::tx(txid);
    let on_the_route = usdt0::MessageLookup {
        guid: Some(second),
        destination_eid: Some(30111),
        ..lookup.clone()
    };
    for (lookup, error) in [
        (lookup, "2 LayerZero messages match"),
        (on_the_route, "identity mismatch"),
    ] {
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"data":[lz_entry(first,txid,"INFLIGHT",30111,json!({})),lz_entry(second,txid,"INFLIGHT",30101,json!({}))]}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let result = api
            .wait_usdt0_by_tx(&lookup, &quick(2000), |d| panic!("reported {d:?}"))
            .await;
        assert!(
            matches!(&result, Err(Error::InvalidSwap(m)) if m.contains(error)),
            "{error}: {result:?}"
        );
        server.verify().await;
    }
}
#[tokio::test]
async fn layerzero_wait_stops_on_statuses_waiting_cannot_resolve() {
    let server = MockServer::start().await;
    let api = BridgeApi::new(&server.uri(), &server.uri()).unwrap();
    let txid = B256::repeat_byte(3);
    for (status, hint) in [
        ("PAYLOAD_STORED", "lzReceive"),
        ("APPLICATION_SKIPPED", "layerzeroscan.com/tx/"),
    ] {
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"data":[lz_entry(B256::repeat_byte(4),txid,status,30111,json!({}))]}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let mut seen = Vec::new();
        let result = api
            .wait_usdt0_by_tx(&usdt0::MessageLookup::tx(txid), &quick(2000), |d| {
                seen.push(d.status)
            })
            .await;
        assert!(
            matches!(&result, Err(Error::InvalidSwap(m)) if m.contains(hint)),
            "{status}: {result:?}"
        );
        assert_eq!(seen.len(), 1);
        server.verify().await;
    }
}
#[tokio::test]
async fn layerzero_unknown_status_keeps_polling() {
    let server = MockServer::start().await;
    let api = BridgeApi::new(&server.uri(), &server.uri()).unwrap();
    let lookup = usdt0::MessageLookup::tx(B256::repeat_byte(3));
    lz_respond(
        &server,
        json!({"data":[lz_entry(B256::repeat_byte(4),lookup.source_txid,"SOMETHING_NEW",30111,json!({}))]}),
    )
    .await;
    let status = api
        .usdt0_status_by_tx(&lookup)
        .await
        .unwrap()
        .unwrap()
        .status;
    assert!(status.is_pending() && !status.needs_intervention());
    assert!(matches!(
        api.wait_usdt0_by_tx(&lookup, &quick(100), |_| {}).await,
        Err(Error::Timeout(m)) if m.contains("last status Unknown")
    ));
}
#[tokio::test]
async fn layerzero_wait_reports_each_status_once() {
    let server = MockServer::start().await;
    let api = BridgeApi::new(&server.uri(), &server.uri()).unwrap();
    let txid = B256::repeat_byte(3);
    for (status, times) in [("INFLIGHT", 2), ("CONFIRMING", 1), ("DELIVERED", 1)] {
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"data":[lz_entry(B256::repeat_byte(4),txid,status,30111,json!({"tx":{"txHash":B256::repeat_byte(8)}}))]}),
            ))
            .up_to_n_times(times)
            .expect(times)
            .mount(&server)
            .await;
    }
    let mut seen = Vec::new();
    let delivery = api
        .wait_usdt0_by_tx(&usdt0::MessageLookup::tx(txid), &quick(2000), |d| {
            seen.push(d.status)
        })
        .await
        .unwrap();
    assert_eq!(delivery.destination_txid, Some(B256::repeat_byte(8)));
    assert_eq!(
        seen,
        [
            usdt0::DeliveryStatus::Inflight,
            usdt0::DeliveryStatus::Confirming,
            usdt0::DeliveryStatus::Delivered
        ]
    );
}
#[tokio::test]
async fn layerzero_wait_retries_read_errors() {
    let server = MockServer::start().await;
    let api = BridgeApi::new(&server.uri(), &server.uri()).unwrap();
    let lookup = usdt0::MessageLookup::tx(B256::repeat_byte(3));
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    assert!(matches!(
        api.usdt0_status_by_tx(&lookup).await,
        Err(Error::Api { status: 429, .. })
    ));
    for status in [400, 500] {
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(status))
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data":[lz_entry(B256::repeat_byte(4),lookup.source_txid,"DELIVERED",30111,json!({}))]}),
        ))
        .mount(&server)
        .await;
    assert_eq!(
        api.wait_usdt0_by_tx(&lookup, &quick(2000), |_| {})
            .await
            .unwrap()
            .guid,
        B256::repeat_byte(4)
    );
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(400))
        .mount(&server)
        .await;
    assert!(matches!(
        api.wait_usdt0_by_tx(&lookup, &quick(100), |_| {}).await,
        Err(Error::Timeout(m)) if m.contains("not indexed") && m.contains("HTTP 400")
    ));
}
#[test]
fn explorer_url_and_chain_lookups_match_the_reference() {
    assert_eq!(
        usdt0::explorer_url(B256::repeat_byte(0xab)),
        format!("https://layerzeroscan.com/tx/0x{}", "ab".repeat(32))
    );
    assert_eq!(usdt0::chain_by_name("Arbitrum").unwrap().eid, 30110);
    assert_eq!(usdt0::chain_by_name("Conflux eSpace").unwrap().eid, 30212);
    assert_eq!(usdt0::chain_by_name("arbitrum"), None);
    assert_eq!(usdt0::chain_by_eid(30111).unwrap().name, "Optimism");
    assert_eq!(usdt0::chain_by_eid(30184), None);
}
