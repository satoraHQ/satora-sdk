//! End-to-end-style unit tests for the HTTP client, using wiremock to stand in
//! for the real backend. These confirm wiring (URL paths, JSON decoding, error
//! mapping) without needing a live API.

use lendaswap_sdk::Client;
use lendaswap_sdk::Error;
use lendaswap_sdk::types::Chain;
use lendaswap_sdk::types::QuoteAmount;
use lendaswap_sdk::types::QuoteRequest;
use lendaswap_sdk::types::TokenId;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[tokio::test]
async fn version_returns_decoded_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "tag": "v0.2.30",
            "commit_hash": "abc123",
        })))
        .mount(&server)
        .await;

    let client = Client::new(&server.uri()).expect("client builds");
    let version = client.version().await.expect("version succeeds");
    assert_eq!(version.tag, "v0.2.30");
    assert_eq!(version.commit_hash, "abc123");
}

#[tokio::test]
async fn health_returns_plain_text_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let client = Client::new(&server.uri()).expect("client builds");
    let body = client.health().await.expect("health succeeds");
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn api_error_body_is_surfaced_in_error_variant() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/version"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "error": "boom",
        })))
        .mount(&server)
        .await;

    let client = Client::new(&server.uri()).expect("client builds");
    let err = client.version().await.expect_err("expected API error");
    match err {
        Error::Api { status, message } => {
            assert_eq!(status, 500);
            assert_eq!(message, "boom");
        }
        other => panic!("expected Error::Api, got {other:?}"),
    }
}

#[tokio::test]
async fn invalid_base_url_returns_typed_error() {
    let err = Client::new("not a url").expect_err("expected URL parse error");
    assert!(matches!(err, Error::InvalidBaseUrl(_)), "got {err:?}");
}

#[tokio::test]
async fn get_quote_builds_expected_query_and_parses_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/quote"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "exchange_rate": "30000.00",
            "network_fee": 1000,
            "gasless_network_fee": 2000,
            "protocol_fee": 250,
            "protocol_fee_rate": 0.0025,
            "min_amount": 10000,
            "max_amount": 100000000,
            "source_amount": "100000",
            "target_amount": "30000000000",
            "net_source_amount": "100000",
            "net_target_amount": "29996750000",
        })))
        .mount(&server)
        .await;

    let client = Client::new(&server.uri()).expect("client builds");
    let req = QuoteRequest::new(
        Chain::bitcoin(),
        TokenId::Btc,
        Chain::polygon(),
        TokenId::UsdcPolygon,
        QuoteAmount::Source(100_000),
    );
    let quote = client.get_quote(req).await.expect("get_quote succeeds");
    assert_eq!(quote.exchange_rate, "30000.00");
    assert_eq!(quote.protocol_fee, 250);
    assert!(quote.bridge_fee.is_none());

    let received = server.received_requests().await.expect("requests recorded");
    let req = received.last().expect("at least one request");
    let params = query_pairs(req);
    assert_eq!(params_get(&params, "source_chain"), Some("Bitcoin"));
    assert_eq!(params_get(&params, "source_token"), Some("btc"));
    assert_eq!(params_get(&params, "target_chain"), Some("137"));
    assert_eq!(
        params_get(&params, "target_token"),
        Some("0x3c499c542cef5e3811e1192ce70d8cc03d5c3359"),
    );
    assert_eq!(params_get(&params, "source_amount"), Some("100000"));
    assert_eq!(params_get(&params, "target_amount"), None);
    assert_eq!(params_get(&params, "bridge_recipient_setup"), Some("false"));
}

#[tokio::test]
async fn get_quote_with_target_amount_omits_source_amount() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/quote"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "exchange_rate": "30000.00",
            "network_fee": 1000,
            "gasless_network_fee": 2000,
            "protocol_fee": 250,
            "protocol_fee_rate": 0.0025,
            "min_amount": 10000,
            "max_amount": 100000000,
            "source_amount": "100000",
            "target_amount": "30000000000",
            "net_source_amount": "100000",
            "net_target_amount": "29996750000",
        })))
        .mount(&server)
        .await;

    let client = Client::new(&server.uri()).expect("client builds");
    let req = QuoteRequest::new(
        Chain::bitcoin(),
        TokenId::Btc,
        Chain::polygon(),
        TokenId::UsdcPolygon,
        QuoteAmount::Target(30_000_000_000),
    );
    client.get_quote(req).await.expect("get_quote succeeds");

    let received = server.received_requests().await.expect("requests recorded");
    let req = received.last().expect("at least one request");
    let params = query_pairs(req);
    assert_eq!(params_get(&params, "source_amount"), None);
    assert_eq!(params_get(&params, "target_amount"), Some("30000000000"));
}

#[tokio::test]
async fn get_quote_sends_optional_bridge_and_referral() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/quote"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "exchange_rate": "30000.00",
            "network_fee": 1000,
            "gasless_network_fee": 2000,
            "protocol_fee": 250,
            "protocol_fee_rate": 0.0025,
            "min_amount": 10000,
            "max_amount": 100000000,
            "source_amount": "100000",
            "target_amount": "30000000000",
            "net_source_amount": "100000",
            "net_target_amount": "29991750000",
            "bridge_fee": 5000,
        })))
        .mount(&server)
        .await;

    let client = Client::new(&server.uri()).expect("client builds");
    let mut req = QuoteRequest::new(
        Chain::bitcoin(),
        TokenId::Btc,
        Chain::polygon(),
        TokenId::UsdcPolygon,
        QuoteAmount::Source(100_000),
    );
    req.bridge_target_chain = Some("Base".to_string());
    req.bridge_recipient_setup = true;
    req.referral_code = Some("FOO123".to_string());

    let quote = client.get_quote(req).await.expect("get_quote succeeds");
    assert_eq!(quote.bridge_fee, Some(5_000));

    let received = server.received_requests().await.expect("requests recorded");
    let req = received.last().expect("at least one request");
    let params = query_pairs(req);
    assert_eq!(params_get(&params, "bridge_target_chain"), Some("Base"));
    assert_eq!(params_get(&params, "bridge_recipient_setup"), Some("true"));
    assert_eq!(params_get(&params, "ref"), Some("FOO123"));
}

// NOTE: a wiremock test for the EVM->Arkade swap flow lived here previously;
// it constructed a `CreateEvmToArkadeSwapRequest` directly. After the
// refactor, the public method takes (source, amount, receive_to) and runs
// signer derivation, so end-to-end testing requires the Phase-2 signer
// crypto. The test will be reintroduced once `Signer::derive_swap_params`
// is implemented.

fn query_pairs(req: &Request) -> Vec<(String, String)> {
    req.url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn params_get<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}
