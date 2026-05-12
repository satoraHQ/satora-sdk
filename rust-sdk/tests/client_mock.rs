//! End-to-end-style unit tests for the HTTP client, using wiremock to stand in
//! for the real backend. These confirm wiring (URL paths, JSON decoding, error
//! mapping) without needing a live API.

use lendaswap_sdk::Client;
use lendaswap_sdk::Error;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
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
