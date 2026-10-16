//! `get_quote` routing, ported from ts-pure-sdk `getQuote`: the exact Lightning
//! send quote, then local composition, then the server's `/quote`.
#[path = "support/quotes.rs"]
mod quotes;
use lendaswap_sdk::Client;
use lendaswap_sdk::Error;
use lendaswap_sdk::types::*;
use quotes::*;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::path;

const ARBITRUM_USDC: &str = "0xaf88d065e77c8cc2239327c5edb3a432268e5831";
const ARBITRUM_USDT0: &str = "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9";
const BASE_USDC: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
const ROOTSTOCK_USDT0: &str = "0x779dED0C9e1022225F8e0630b35A9B54Be713736";
const RBTC: &str = "0x0000000000000000000000000000000000000000";
const SOLANA_USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

fn server_quote() -> QuoteResponse {
    QuoteResponse {
        exchange_rate: "server".into(),
        network_fee: 1,
        gasless_network_fee: 0,
        protocol_fee: 2,
        protocol_fee_rate: 0.003,
        min_amount: 1_000,
        max_amount: 1_000_000,
        source_amount: "1".into(),
        target_amount: "1".into(),
        net_source_amount: "1".into(),
        net_target_amount: "1".into(),
        bridge_fee: None,
    }
}

fn lightning_send(source_amount: &str, exchange_rate: &str) -> Value {
    json!({
        "exchange_rate": exchange_rate, "source_amount": source_amount,
        "source_amount_sats": 100_250, "target_amount_sats": 100_000, "protocol_fee_sats": 200,
        "network_fee_sats": 50, "protocol_fee_rate": 0.002, "min_amount_sats": 1_000,
        "max_amount_sats": 1_000_000
    })
}

fn ok(body: impl serde::Serialize) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

async fn mount(server: &MockServer, route: &str, response: ResponseTemplate) {
    Mock::given(path(route))
        .respond_with(response)
        .mount(server)
        .await;
}

async fn mount_discovery(server: &MockServer) {
    let (pairs, fees, config) = data();
    mount(server, "/swap-pairs", ok(pairs)).await;
    mount(server, "/network-fees", ok(fees)).await;
    mount(server, "/chain-config", ok(config)).await;
}

async fn paths(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect()
}

async fn query(server: &MockServer, route: &str) -> HashMap<String, String> {
    let requests = server.received_requests().await.unwrap();
    let request = requests
        .iter()
        .rev()
        .find(|request| request.url.path() == route)
        .unwrap();
    request.url.query_pairs().into_owned().collect()
}

async fn quote_fields(server: &MockServer, keys: &[&str]) -> Vec<String> {
    let query = query(server, "/quote").await;
    keys.iter()
        .map(|key| query.get(*key).cloned().unwrap_or_default())
        .collect()
}

const TARGET: [&str; 3] = ["target_chain", "target_token", "bridge_target_chain"];
const SOURCE: [&str; 3] = ["source_chain", "source_token", "bridge_source_chain"];

fn client(server: &MockServer) -> Client {
    let mut client = Client::new(&server.uri()).unwrap();
    client.set_referral_code(Some("default".into()));
    client
}

fn to_lightning(source_chain: Chain, source_token: TokenId, amount: QuoteAmount) -> QuoteRequest {
    QuoteRequest::new(
        source_chain,
        source_token,
        Chain::lightning(),
        TokenId::Btc,
        amount,
    )
}

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Logs {
    fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
        let logs = Self::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }
    fn take(&self) -> String {
        String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap()
    }
}

#[tokio::test]
async fn arkade_address_quote_comes_from_the_exact_lightning_send_endpoint() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/quote/lightning-send",
        ok(lightning_send("100250", "1")),
    )
    .await;
    let mut request = to_lightning(Chain::arkade(), TokenId::Btc, QuoteAmount::Target(100_000))
        .with_lightning_destination("alice@example.com");
    request.referral_code = Some("override".into());
    request.extra_fees_bps = Some(20);
    let quote = client(&server).get_quote(request).await.unwrap();
    assert_eq!(
        quote,
        QuoteResponse {
            exchange_rate: "1".into(),
            network_fee: 50,
            gasless_network_fee: 0,
            protocol_fee: 200,
            protocol_fee_rate: 0.002,
            min_amount: 1_000,
            max_amount: 1_000_000,
            source_amount: "100000".into(),
            target_amount: "100000".into(),
            net_source_amount: "100250".into(),
            net_target_amount: "100000".into(),
            bridge_fee: None,
        }
    );
    assert_eq!(paths(&server).await, ["/quote/lightning-send"]);
    let query = query(&server, "/quote/lightning-send").await;
    assert_eq!(query["source_chain"], "Arkade");
    assert_eq!(query["lightning_address"], "alice@example.com");
    assert_eq!(query["target_amount_sats"], "100000");
    assert_eq!(query["ref"], "override");
    assert_eq!(query["extra_fees"], "20");
    assert!(!query.contains_key("source_amount"));
    assert!(!query.contains_key("source_token"));
}

#[tokio::test]
async fn evm_send_max_quote_keeps_the_token_amount_and_dex_rate() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/quote/lightning-send",
        ok(lightning_send("5000000", "60000.5")),
    )
    .await;
    let request = to_lightning(
        Chain::polygon(),
        TokenId::UsdcPolygon,
        QuoteAmount::Source(5_000_000),
    )
    .with_lightning_destination("LNURL1DP68GURN8GHJ7");
    let quote = client(&server).get_quote(request).await.unwrap();
    assert_eq!(quote.exchange_rate, "60000.5");
    assert_eq!(quote.source_amount, "5000000");
    assert_eq!(quote.target_amount, "100000");
    assert_eq!(quote.net_source_amount, "5000000");
    assert_eq!(quote.net_target_amount, "100000");
    let query = query(&server, "/quote/lightning-send").await;
    assert_eq!(query["source_chain"], "137");
    assert_eq!(query["source_token"], TokenId::UsdcPolygon.as_wire_str());
    assert_eq!(query["source_amount"], "5000000");
    assert_eq!(query["lnurl"], "LNURL1DP68GURN8GHJ7");
    assert_eq!(query["ref"], "default");
    assert!(!query.contains_key("target_amount_sats"));
}

#[tokio::test]
async fn evm_exact_and_invoice_quotes_report_the_token_amount_to_fund() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/quote/lightning-send",
        ok(lightning_send("42000000", "41895.26")),
    )
    .await;
    let client = client(&server);
    let exact = to_lightning(
        Chain::polygon(),
        TokenId::UsdcPolygon,
        QuoteAmount::Target(100_000),
    )
    .with_lightning_destination("user@example.test");
    let quote = client.get_quote(exact).await.unwrap();
    assert_eq!(quote.exchange_rate, "41895.26");
    assert_eq!(quote.source_amount, "42000000");
    assert_eq!(quote.target_amount, "100000");
    assert_eq!(quote.net_source_amount, "42000000");
    assert_eq!(quote.net_target_amount, "100000");
    let query_of_exact = query(&server, "/quote/lightning-send").await;
    assert_eq!(query_of_exact["source_chain"], "137");
    assert_eq!(query_of_exact["lightning_address"], "user@example.test");
    assert_eq!(query_of_exact["target_amount_sats"], "100000");
    assert!(!query_of_exact.contains_key("source_amount"));

    let invoice = to_lightning(
        Chain::polygon(),
        TokenId::UsdcPolygon,
        QuoteAmount::Target(5),
    )
    .with_lightning_destination("lnbc1fixture");
    let quote = client.get_quote(invoice).await.unwrap();
    assert_eq!(quote.source_amount, "42000000");
    assert_eq!(quote.target_amount, "100000");
    let query_of_invoice = query(&server, "/quote/lightning-send").await;
    assert_eq!(query_of_invoice["lightning_invoice"], "lnbc1fixture");
    assert!(!query_of_invoice.contains_key("target_amount_sats"));
    assert!(!query_of_invoice.contains_key("source_amount"));
    assert_eq!(
        paths(&server).await,
        ["/quote/lightning-send", "/quote/lightning-send"]
    );
}

#[tokio::test]
async fn arkade_send_max_and_invoice_amounts_follow_the_typescript_adapter() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/quote/lightning-send",
        ok(lightning_send("100250", "1")),
    )
    .await;
    let client = client(&server);
    let send_max = to_lightning(Chain::arkade(), TokenId::Btc, QuoteAmount::Source(100_250))
        .with_lightning_destination("alice@example.com");
    let quote = client.get_quote(send_max).await.unwrap();
    assert_eq!(quote.source_amount, "100250");
    assert_eq!(quote.target_amount, "100250");
    assert_eq!(quote.net_source_amount, "100250");
    assert_eq!(quote.net_target_amount, "100000");
    assert_eq!(
        query(&server, "/quote/lightning-send").await["source_amount"],
        "100250"
    );

    let invoice = to_lightning(Chain::arkade(), TokenId::Btc, QuoteAmount::Source(5))
        .with_lightning_destination("lnbcrt1fixture");
    let quote = client.get_quote(invoice).await.unwrap();
    assert_eq!(quote.source_amount, "100000");
    assert_eq!(quote.target_amount, "100000");
    assert_eq!(quote.net_source_amount, "100250");
    let query = query(&server, "/quote/lightning-send").await;
    assert_eq!(query["lightning_invoice"], "lnbcrt1fixture");
    assert!(!query.contains_key("target_amount_sats"));
    assert!(!query.contains_key("source_amount"));
}

#[tokio::test]
async fn failed_or_unusable_lightning_send_quotes_fall_back_to_the_server() {
    let (logs, _guard) = Logs::capture();
    let server = MockServer::start().await;
    mount(
        &server,
        "/quote/lightning-send",
        ResponseTemplate::new(503).set_body_json(json!({"error": "lightning unavailable"})),
    )
    .await;
    mount(&server, "/quote", ok(server_quote())).await;
    let client = client(&server);
    let request = to_lightning(Chain::arkade(), TokenId::Btc, QuoteAmount::Target(100_000))
        .with_lightning_destination("alice@example.com");
    assert_eq!(client.get_quote(request).await.unwrap(), server_quote());
    assert_eq!(paths(&server).await, ["/quote/lightning-send", "/quote"]);
    assert!(
        logs.take()
            .contains("client.getQuote.lightningSendFallback")
    );

    server.reset().await;
    mount(&server, "/quote", ok(server_quote())).await;
    for request in [
        to_lightning(Chain::arkade(), TokenId::Btc, QuoteAmount::Target(100_000))
            .with_lightning_destination("bc1qnotlightning"),
        to_lightning(Chain::bitcoin(), TokenId::Btc, QuoteAmount::Target(100_000))
            .with_lightning_destination("alice@example.com"),
    ] {
        assert_eq!(client.get_quote(request).await.unwrap(), server_quote());
    }
    assert_eq!(paths(&server).await, ["/quote", "/quote"]);
    assert_eq!(query(&server, "/quote").await["ref"], "default");
}

#[tokio::test]
async fn native_rbtc_pays_lightning_through_the_exact_quote() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/quote/lightning-send",
        ok(lightning_send("100250", "1")),
    )
    .await;
    let request = to_lightning(
        Chain::rootstock(),
        TokenId::Other(RBTC.into()),
        QuoteAmount::Target(100_000),
    )
    .with_lightning_destination("alice@example.com");
    let quote = client(&server).get_quote(request).await.unwrap();
    assert_eq!(quote.net_source_amount, "100250");
    assert_eq!(paths(&server).await, ["/quote/lightning-send"]);
    let query = query(&server, "/quote/lightning-send").await;
    assert_eq!(query["source_chain"], "30");
    assert_eq!(query["source_token"], RBTC);
}

#[tokio::test]
async fn only_lightning_targets_from_priced_sources_ask_for_a_lightning_send_quote() {
    let server = MockServer::start().await;
    Mock::given(path("/quote/lightning-send"))
        .respond_with(ok(lightning_send("100250", "1")))
        .expect(0)
        .mount(&server)
        .await;
    mount(&server, "/quote", ok(server_quote())).await;
    let client = client(&server);
    let rootstock_usdt0 = to_lightning(
        Chain::rootstock(),
        TokenId::Other(ROOTSTOCK_USDT0.into()),
        QuoteAmount::Target(100_000),
    )
    .with_lightning_destination("alice@example.com");
    let not_lightning = QuoteRequest::new(
        Chain::arkade(),
        TokenId::Btc,
        Chain::arbitrum(),
        TokenId::UsdcArbitrum,
        QuoteAmount::Target(1_000_000),
    )
    .with_lightning_destination("alice@example.com");
    for request in [rootstock_usdt0, not_lightning] {
        assert_eq!(client.get_quote(request).await.unwrap(), server_quote());
    }
    assert!(
        !paths(&server)
            .await
            .contains(&"/quote/lightning-send".to_owned())
    );
}

#[tokio::test]
async fn btc_to_evm_quotes_are_composed_without_the_server() {
    let server = MockServer::start().await;
    mount_discovery(&server).await;
    Mock::given(path("/quote"))
        .respond_with(ok(server_quote()))
        .expect(0)
        .mount(&server)
        .await;
    let client = Client::new(&server.uri()).unwrap();
    let request = request(true, Chain::arbitrum(), TBTC, QuoteAmount::Source(100_000));
    let (pairs, fees, config) = data();
    let expected = client
        .compose_quote_with_data(request.clone(), 100, &pairs, &fees, &config, 0.0)
        .await
        .unwrap();
    assert_eq!(client.get_quote(request).await.unwrap(), expected);
    assert_eq!(expected.target_amount, "1000000000000000");
}

#[tokio::test]
async fn evm_to_btc_and_btc_to_solana_quotes_are_composed_too() {
    let server = MockServer::start().await;
    mount_discovery(&server).await;
    mount(
        &server,
        "/dex-quote",
        ok(json!({
            "expected_amount_in": {"raw": "10000000000000000", "decimals": 18},
            "estimated_amount_out": {"raw": "990000", "decimals": 6}, "bridge_fee": "10000",
            "estimated_settlement_seconds": 15, "router": "cctp", "cache_ttl_seconds": 30
        })),
    )
    .await;
    Mock::given(path("/quote"))
        .respond_with(ok(server_quote()))
        .expect(0)
        .mount(&server)
        .await;
    let client = Client::new(&server.uri()).unwrap();
    let (pairs, fees, config) = data();
    for request in [
        request(
            false,
            Chain::arbitrum(),
            TBTC,
            QuoteAmount::Source(1_000_000_000_000_000),
        ),
        request(
            true,
            Chain::Other("Solana".into()),
            SOLANA_USDC,
            QuoteAmount::Source(1_000_000),
        ),
    ] {
        let expected = client
            .compose_quote_with_data(request.clone(), 100, &pairs, &fees, &config, 0.0)
            .await
            .unwrap();
        assert_eq!(client.get_quote(request).await.unwrap(), expected);
    }
    let requests = server.received_requests().await.unwrap();
    let dex = requests
        .iter()
        .rfind(|request| request.url.path() == "/dex-quote")
        .unwrap();
    let body: Value = serde_json::from_slice(&dex.body).unwrap();
    assert_eq!(body["to"]["kind"], "solana");
    assert!(!paths(&server).await.contains(&"/quote".to_owned()));
}

#[tokio::test]
async fn composed_dex_quotes_use_the_default_slippage() {
    let server = MockServer::start().await;
    mount_discovery(&server).await;
    mount(
        &server,
        "/dex-quote",
        ok(json!({
            "expected_amount_in": {"raw": "1000000000000000", "decimals": 18},
            "estimated_amount_out": {"raw": "2000000", "decimals": 6},
            "estimated_settlement_seconds": 0, "router": "uniswap", "cache_ttl_seconds": 30
        })),
    )
    .await;
    let client = Client::new(&server.uri()).unwrap();
    let quote = client
        .get_quote(request(
            true,
            Chain::arbitrum(),
            USDC,
            QuoteAmount::Source(100_000),
        ))
        .await
        .unwrap();
    assert_eq!(quote.target_amount, "2000000");
    let requests = server.received_requests().await.unwrap();
    let dex = requests
        .iter()
        .find(|request| request.url.path() == "/dex-quote")
        .unwrap();
    let body: Value = serde_json::from_slice(&dex.body).unwrap();
    assert_eq!(body["slippage_bps"], 100);
    assert!(!paths(&server).await.contains(&"/quote".to_owned()));
}

#[tokio::test]
async fn compose_failures_fall_back_and_only_unexpected_ones_are_logged() {
    let (logs, _guard) = Logs::capture();
    let server = MockServer::start().await;
    mount_discovery(&server).await;
    mount(&server, "/dex-quote", ResponseTemplate::new(500)).await;
    mount(&server, "/quote", ok(server_quote())).await;
    let client = Client::new(&server.uri()).unwrap();
    let dex_route = request(true, Chain::arbitrum(), USDC, QuoteAmount::Source(100_000));
    assert_eq!(client.get_quote(dex_route).await.unwrap(), server_quote());
    assert_eq!(paths(&server).await.last().unwrap(), "/quote");
    assert!(logs.take().contains("client.getQuote.composeFallback"));

    // The fixtures have no Lightning pair, which composition reports as unsupported.
    let mut unpaired = request(true, Chain::arbitrum(), TBTC, QuoteAmount::Source(100_000));
    unpaired.source_chain = Chain::lightning();
    assert_eq!(client.get_quote(unpaired).await.unwrap(), server_quote());
    assert_eq!(paths(&server).await.last().unwrap(), "/quote");
    assert!(!logs.take().contains("client.getQuote"));
}

#[tokio::test]
async fn routes_composition_cannot_serve_go_straight_to_the_server() {
    let server = MockServer::start().await;
    mount(&server, "/quote", ok(server_quote())).await;
    let client = Client::new(&server.uri()).unwrap();
    for request in [
        QuoteRequest::new(
            Chain::lightning(),
            TokenId::Btc,
            Chain::arkade(),
            TokenId::Btc,
            QuoteAmount::Source(100_000),
        ),
        QuoteRequest::new(
            Chain::polygon(),
            TokenId::UsdcPolygon,
            Chain::arbitrum(),
            TokenId::UsdcArbitrum,
            QuoteAmount::Source(100_000),
        ),
        request(
            false,
            Chain::Other("Solana".into()),
            SOLANA_USDC,
            QuoteAmount::Source(1_000_000),
        ),
    ] {
        assert_eq!(client.get_quote(request).await.unwrap(), server_quote());
    }
    assert_eq!(paths(&server).await, ["/quote", "/quote", "/quote"]);
    assert_eq!(
        quote_fields(&server, &SOURCE).await,
        ["Solana", SOLANA_USDC, ""]
    );
}

#[tokio::test]
async fn explicit_bridge_fields_skip_composition_and_the_remap() {
    let server = MockServer::start().await;
    mount(&server, "/quote", ok(server_quote())).await;
    let client = Client::new(&server.uri()).unwrap();
    let mut to_base = request(
        true,
        Chain::Other("8453".into()),
        BASE_USDC,
        QuoteAmount::Source(100_000),
    );
    to_base.bridge_target_chain = Some("Base".into());
    client.get_quote(to_base).await.unwrap();
    assert_eq!(
        quote_fields(&server, &TARGET).await,
        ["8453", BASE_USDC, "Base"]
    );

    let mut from_base = request(
        false,
        Chain::Other("8453".into()),
        BASE_USDC,
        QuoteAmount::Source(1_000_000),
    );
    from_base.bridge_source_chain = Some("Base".into());
    client.get_quote(from_base).await.unwrap();
    assert_eq!(
        quote_fields(&server, &SOURCE).await,
        ["8453", BASE_USDC, "Base"]
    );
    assert_eq!(paths(&server).await, ["/quote", "/quote"]);
}

#[tokio::test]
async fn server_quotes_move_bridge_only_chains_to_arbitrum() {
    let server = MockServer::start().await;
    mount(&server, "/swap-pairs", ResponseTemplate::new(500)).await;
    mount(&server, "/quote", ok(server_quote())).await;
    let client = Client::new(&server.uri()).unwrap();
    let base = request(
        true,
        Chain::Other("8453".into()),
        BASE_USDC,
        QuoteAmount::Source(100_000),
    );
    assert_eq!(client.get_quote(base).await.unwrap(), server_quote());
    assert_eq!(
        quote_fields(&server, &TARGET).await,
        ["42161", ARBITRUM_USDC, "Base"]
    );

    for (chain, token, expected) in [
        (
            Chain::rootstock(),
            ROOTSTOCK_USDT0,
            ["42161", ARBITRUM_USDT0, "Rootstock"],
        ),
        (
            Chain::Other("Solana".into()),
            SOLANA_USDC,
            ["42161", ARBITRUM_USDC, "Solana"],
        ),
        (Chain::rootstock(), RBTC, ["30", RBTC, ""]),
    ] {
        let request = request(true, chain, token, QuoteAmount::Source(100_000));
        client.get_server_quote(request).await.unwrap();
        assert_eq!(quote_fields(&server, &TARGET).await, expected);
    }

    let base_source = request(
        false,
        Chain::Other("8453".into()),
        BASE_USDC,
        QuoteAmount::Source(1_000_000),
    );
    client.get_server_quote(base_source).await.unwrap();
    assert_eq!(
        quote_fields(&server, &SOURCE).await,
        ["42161", ARBITRUM_USDC, "Base"]
    );

    let requests = server.received_requests().await.unwrap().len();
    let not_usdc = request(
        false,
        Chain::Other("8453".into()),
        ROOTSTOCK_USDT0,
        QuoteAmount::Source(1_000_000),
    );
    assert!(matches!(
        client.get_server_quote(not_usdc).await,
        Err(Error::InvalidSwap(_))
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), requests);
}
