//! End-to-end-style unit tests for the HTTP client, using wiremock to stand in
//! for the real backend. These confirm wiring (URL paths, JSON decoding, error
//! mapping) without needing a live API.

use lendaswap_sdk::Client;
use lendaswap_sdk::Error;
use lendaswap_sdk::SwapFunding;
use lendaswap_sdk::types::Address;
use lendaswap_sdk::types::Chain;
use lendaswap_sdk::types::KnownChain;
use lendaswap_sdk::types::QuoteAmount;
use lendaswap_sdk::types::QuoteRequest;
use lendaswap_sdk::types::SwapStatus;
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

/// Well-known BIP-39 test mnemonic — produces deterministic signer
/// output so the wiremock test can verify exact wire shapes.
const TEST_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

#[tokio::test]
async fn create_evm_to_arkade_swap_posts_high_level_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/swap/evm/arkade"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "swap_42",
            "status": "pending",
            "fee_sats": 500,
            "hash_lock": "0xdeadbeef",
            "source_token": {
                "token_id": "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9",
                "symbol": "USDT",
                "chain": "42161",
                "name": "Tether USD",
                "decimals": 6,
            },
            "target_token": {
                "token_id": "btc",
                "symbol": "BTC",
                "chain": "Arkade",
                "name": "Bitcoin",
                "decimals": 8,
            },
            "created_at": "2026-05-12T00:00:00Z",
            "chain": "Arbitrum",
            "evm_chain_id": 42161,
            "source_amount": "100000000",
            "target_amount": "150000",
            "evm_expected_sats": "150000",
            "evm_htlc_address": "0xhtlc",
            "client_evm_address": "0xclient",
            "server_evm_address": "0xserver",
            "evm_refund_locktime": 1_000_000,
            "btc_vhtlc_address": "ark1qvhtlc",
            "target_arkade_address": "ark1qtarget",
            "sender_pk": "02sender",
            "receiver_pk": "02receiver",
            "arkade_server_pk": "02arkade",
            "vhtlc_refund_locktime": 1_000_000,
            "unilateral_claim_delay": 144,
            "unilateral_refund_delay": 288,
            "unilateral_refund_without_receiver_delay": 432,
            "network": "mainnet",
            "gasless": true,
        })))
        .mount(&server)
        .await;

    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(TEST_MNEMONIC)
        .referral_code("TEST_REF")
        .build()
        .expect("client builds");

    let swap = client
        .create_evm_to_arkade_swap(
            TokenId::Usdt0Arbitrum,
            QuoteAmount::Source(100_000_000),
            Address::Arkade("ark1qtarget".to_string()),
            true,
            None,
        )
        .await
        .expect("create_evm_to_arkade_swap succeeds");

    assert_eq!(swap.id, "swap_42");
    assert_eq!(swap.status, SwapStatus::Pending);
    assert_eq!(swap.deposit_token, TokenId::Usdt0Arbitrum);
    assert_eq!(swap.receive_token, TokenId::Btc);
    assert_eq!(swap.receive_address, "ark1qtarget");
    // gasless=true → Gasless variant with the SDK-derived EVM address
    // (echoed back as client_evm_address by the backend).
    match swap.funding {
        SwapFunding::Gasless { deposit_address } => {
            assert_eq!(deposit_address, "0xclient");
        }
        other => panic!("expected Gasless funding, got {other:?}"),
    }

    // Inspect the wire body to confirm the signer-derived fields and the
    // builder-set referral code flow through.
    let received = server.received_requests().await.expect("requests recorded");
    let req = received.last().expect("at least one request");
    let body: serde_json::Value = serde_json::from_slice(&req.body).expect("body is valid JSON");
    assert_eq!(body["target_address"], "ark1qtarget");
    assert_eq!(body["evm_chain_id"], 42161);
    assert_eq!(
        body["token_address"],
        "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9"
    );
    assert_eq!(body["amount_in"], 100_000_000);
    assert!(body.get("amount_out").is_none() || body["amount_out"].is_null());
    assert_eq!(body["gasless"], true);
    // Swap-request body uses `referral_code` (the quote endpoint renames it
    // to `ref`; the two endpoints differ on this field name).
    assert_eq!(body["referral_code"], "TEST_REF");
    // hash_lock + receiver_pk + user_id + user_address come from the
    // signer; check shape, not exact value (those are well-defined by the
    // signer unit tests).
    assert!(body["hash_lock"].as_str().unwrap().starts_with("0x"));
    assert_eq!(body["hash_lock"].as_str().unwrap().len(), 66); // 0x + 64 hex
    assert_eq!(body["receiver_pk"].as_str().unwrap().len(), 66); // 33 bytes hex
    assert_eq!(body["user_id"].as_str().unwrap().len(), 66);
    assert!(body["user_address"].as_str().unwrap().starts_with("0x"));
}

#[tokio::test]
async fn create_swap_dispatcher_routes_to_evm_to_arkade() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/swap/evm/arkade"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "swap_99",
            "status": "pending",
            "fee_sats": 500,
            "hash_lock": "0xdeadbeef",
            "source_token": {
                "token_id": "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359",
                "symbol": "USDC",
                "chain": "137",
                "name": "USD Coin",
                "decimals": 6,
            },
            "target_token": {
                "token_id": "btc",
                "symbol": "BTC",
                "chain": "Arkade",
                "name": "Bitcoin",
                "decimals": 8,
            },
            "created_at": "2026-05-12T00:00:00Z",
            "chain": "Polygon",
            "evm_chain_id": 137,
            "source_amount": "100000000",
            "target_amount": "150000",
            "evm_expected_sats": "150000",
            "evm_htlc_address": "0xhtlc",
            "client_evm_address": "0xclient",
            "server_evm_address": "0xserver",
            "evm_refund_locktime": 1_000_000,
            "btc_vhtlc_address": "ark1qvhtlc",
            "target_arkade_address": "ark1qtarget",
            "sender_pk": "02sender",
            "receiver_pk": "02receiver",
            "arkade_server_pk": "02arkade",
            "vhtlc_refund_locktime": 1_000_000,
            "unilateral_claim_delay": 144,
            "unilateral_refund_delay": 288,
            "unilateral_refund_without_receiver_delay": 432,
            "network": "mainnet",
            "gasless": false,
        })))
        .mount(&server)
        .await;

    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(TEST_MNEMONIC)
        .build()
        .expect("client builds");

    let swap = client
        .create_swap(
            KnownChain::Polygon,
            TokenId::UsdcPolygon,
            TokenId::Btc,
            QuoteAmount::Source(100_000_000),
            Some(Address::Arkade("ark1qtarget".to_string())),
            None,
        )
        .await
        .expect("dispatcher routes to evm_to_arkade");
    assert_eq!(swap.id, "swap_99");
}

#[tokio::test]
async fn create_swap_rejects_unsupported_direction() {
    let client = Client::builder()
        .base_url("https://example.invalid")
        .mnemonic(TEST_MNEMONIC)
        .build()
        .expect("client builds");

    // Bitcoin (on-chain) source -> EVM target is not wired today.
    let err = client
        .create_swap(
            KnownChain::Bitcoin,
            TokenId::Btc,
            TokenId::UsdcPolygon,
            QuoteAmount::Source(100_000),
            Some(Address::Evm("0xclient".to_string())),
            None,
        )
        .await
        .expect_err("unsupported direction must error");
    assert!(matches!(err, Error::InvalidSwap(_)), "got {err:?}");
}

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
