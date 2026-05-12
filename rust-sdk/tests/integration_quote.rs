//! Live integration tests for `GET /quote`.
//!
//! Disabled by default (`#[ignore]`) — these hit a real Lendaswap server, so
//! `cargo test` stays hermetic. Run manually:
//!
//! ```bash
//! cargo test --test integration_quote -- --ignored --nocapture
//!
//! LENDASWAP_API_URL=https://api.satora.io \
//!   cargo test --test integration_quote -- --ignored --nocapture
//! ```
//!
//! Each `matrix_*` test loops over a small Cartesian product of source-chain ×
//! target-chain (× token) and asserts the structural invariants every quote
//! must satisfy. On failure the panic message names the exact pair that
//! broke, so identifying which leg of the matrix went wrong takes one glance.

use lendaswap_sdk::Client;
use lendaswap_sdk::types::Chain;
use lendaswap_sdk::types::KnownChain;
use lendaswap_sdk::types::QuoteAmount;
use lendaswap_sdk::types::QuoteRequest;
use lendaswap_sdk::types::QuoteResponse;
use lendaswap_sdk::types::TokenId;
use lendaswap_sdk::types::well_known;

const DEFAULT_BASE_URL: &str = "http://localhost:3333";

/// Default amount used by the BTC-source matrices: 100_000 sats (~$60 at $60k BTC).
/// Chosen to clear `min_amount` for every supported pair while staying well below
/// `max_amount` so quotes return rather than 400-ing on bounds.
const DEFAULT_SOURCE_SATS: u64 = 100_000;

/// Default amount used by the stable-source matrices: 100 USDC/USDT (6 decimals).
/// Equivalent to ~150k sats at $60k BTC — comfortably inside every supported
/// pair's `[min_amount, max_amount]` window.
const DEFAULT_SOURCE_STABLE_RAW: u64 = 100_000_000;

fn base_url() -> String {
    std::env::var("LENDASWAP_API_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
}

fn client() -> Client {
    Client::new(&base_url()).expect("base URL parses")
}

/// Set up a tracing subscriber that writes to the test harness's captured
/// output (visible with `cargo test -- --nocapture`). Idempotent — every
/// `try_init` after the first returns Err and we ignore it, which keeps
/// multi-test runs from fighting for the global subscriber.
///
/// Default filter is `lendaswap_sdk=debug,info`; override with `RUST_LOG`.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("lendaswap_sdk=debug,info")),
        )
        .with_test_writer()
        .try_init();
}

/// The three BTC-bearing chains. Used as `source_chain` in BTC→stable
/// matrices and as `target_chain` in stable→BTC matrices. All pair with
/// `TokenId::btc()` — the chain distinguishes the on-ramp (on-chain BTC vs
/// Lightning vs Arkade VTXO).
fn btc_chains() -> [KnownChain; 3] {
    [
        KnownChain::Bitcoin,
        KnownChain::Lightning,
        KnownChain::Arkade,
    ]
}

/// The three EVM chains for which the SDK ships known USDC / USDT addresses.
fn evm_chains() -> [KnownChain; 3] {
    [
        KnownChain::Polygon,
        KnownChain::Arbitrum,
        KnownChain::Ethereum,
    ]
}

fn make_request(source_chain: Chain, target_chain: Chain, target_token: TokenId) -> QuoteRequest {
    QuoteRequest::new(
        source_chain,
        TokenId::btc(),
        target_chain,
        target_token,
        QuoteAmount::Source(DEFAULT_SOURCE_SATS),
    )
}

/// Mirror of [`make_request`] for the reverse direction: a stablecoin source
/// settles into BTC.
fn make_reverse_request(
    source_chain: Chain,
    source_token: TokenId,
    target_chain: Chain,
) -> QuoteRequest {
    QuoteRequest::new(
        source_chain,
        source_token,
        target_chain,
        TokenId::btc(),
        QuoteAmount::Source(DEFAULT_SOURCE_STABLE_RAW),
    )
}

/// Structural invariants every successful quote must satisfy. Called after
/// every quote in the matrix tests below.
fn assert_quote_invariants(req: &QuoteRequest, resp: &QuoteResponse) {
    assert!(resp.protocol_fee > 0, "protocol_fee should be > 0");
    // `network_fee` covers server-paid gas / BTC mining fees and is legitimately
    // 0 for fully off-chain pairs (Lightning ↔ Arkade), so we don't require > 0.
    assert!(
        resp.min_amount <= resp.max_amount,
        "min_amount ({}) must be <= max_amount ({})",
        resp.min_amount,
        resp.max_amount,
    );

    let rate: f64 = resp
        .exchange_rate
        .parse()
        .unwrap_or_else(|_| panic!("exchange_rate failed to parse: {:?}", resp.exchange_rate));
    assert!(rate > 0.0, "exchange_rate must be positive, got {rate}");

    let source_amount = parse_u128("source_amount", &resp.source_amount);
    let target_amount = parse_u128("target_amount", &resp.target_amount);
    let net_source = parse_u128("net_source_amount", &resp.net_source_amount);
    let net_target = parse_u128("net_target_amount", &resp.net_target_amount);

    match req.amount {
        QuoteAmount::Source(n) => {
            let n = u128::from(n);
            assert_eq!(
                source_amount, n,
                "Source mode: source_amount should equal requested"
            );
            assert_eq!(
                net_source, n,
                "Source mode: net_source_amount should equal requested"
            );
            assert!(
                net_target <= target_amount,
                "Source mode: net_target_amount ({net_target}) <= target_amount ({target_amount})",
            );
        }
        QuoteAmount::Target(n) => {
            let n = u128::from(n);
            assert_eq!(
                target_amount, n,
                "Target mode: target_amount should equal requested"
            );
            assert_eq!(
                net_target, n,
                "Target mode: net_target_amount should equal requested"
            );
            assert!(
                net_source >= source_amount,
                "Target mode: net_source_amount ({net_source}) >= source_amount ({source_amount})",
            );
        }
    }

    assert_eq!(
        resp.bridge_fee.is_some(),
        req.bridge_target_chain.is_some(),
        "bridge_fee presence must mirror bridge_target_chain",
    );
}

fn parse_u128(field: &str, s: &str) -> u128 {
    s.parse()
        .unwrap_or_else(|_| panic!("{field} should parse as u128, got {s:?}"))
}

// ─────────────────────────────────────────────────────────────────────────────
// Matrix 1: BTC sources → USDC on each EVM chain (9 combos)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "live: requires a running Lendaswap server (default http://localhost:3333)"]
async fn matrix_btc_sources_to_usdc_on_evm_chains() {
    init_tracing();
    let client = client();
    for source in btc_chains() {
        for target_chain in evm_chains() {
            let usdc = well_known::usdc(target_chain.clone())
                .unwrap_or_else(|| panic!("expected known USDC on {target_chain:?}"));
            let req = make_request(
                Chain::Known(source.clone()),
                Chain::Known(target_chain.clone()),
                usdc,
            );
            let resp = client
                .get_quote(req.clone())
                .await
                .unwrap_or_else(|e| panic!("quote {source:?} -> USDC@{target_chain:?}: {e:?}"));
            assert_quote_invariants(&req, &resp);
            tracing::info!(
                ?source,
                ?target_chain,
                rate = %resp.exchange_rate,
                protocol_fee = resp.protocol_fee,
                net_target = %resp.net_target_amount,
                "BTC@{source:?} -> USDC@{target_chain:?}",
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Matrix 2: BTC sources → USDT on each EVM chain (9 combos)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "live: requires a running Lendaswap server (default http://localhost:3333)"]
async fn matrix_btc_sources_to_usdt_on_evm_chains() {
    init_tracing();
    let client = client();
    for source in btc_chains() {
        for target_chain in evm_chains() {
            let usdt = well_known::usdt(target_chain.clone())
                .unwrap_or_else(|| panic!("expected known USDT on {target_chain:?}"));
            let req = make_request(
                Chain::Known(source.clone()),
                Chain::Known(target_chain.clone()),
                usdt,
            );
            let resp = client
                .get_quote(req.clone())
                .await
                .unwrap_or_else(|e| panic!("quote {source:?} -> USDT@{target_chain:?}: {e:?}"));
            assert_quote_invariants(&req, &resp);
            tracing::info!(
                ?source,
                ?target_chain,
                rate = %resp.exchange_rate,
                protocol_fee = resp.protocol_fee,
                net_target = %resp.net_target_amount,
                "BTC@{source:?} -> USDT@{target_chain:?}",
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Matrix 3: USDC on each EVM chain → BTC targets (9 combos)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "live: requires a running Lendaswap server (default http://localhost:3333)"]
async fn matrix_usdc_on_evm_chains_to_btc_targets() {
    init_tracing();
    let client = client();
    for source_chain in evm_chains() {
        let usdc = well_known::usdc(source_chain.clone())
            .unwrap_or_else(|| panic!("expected known USDC on {source_chain:?}"));
        for target in btc_chains() {
            let req = make_reverse_request(
                Chain::Known(source_chain.clone()),
                usdc.clone(),
                Chain::Known(target.clone()),
            );
            let resp = client
                .get_quote(req.clone())
                .await
                .unwrap_or_else(|e| panic!("quote USDC@{source_chain:?} -> {target:?}: {e:?}"));
            assert_quote_invariants(&req, &resp);
            tracing::info!(
                ?source_chain,
                ?target,
                rate = %resp.exchange_rate,
                protocol_fee = resp.protocol_fee,
                net_target = %resp.net_target_amount,
                "USDC@{source_chain:?} -> BTC@{target:?}",
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Matrix 4: USDT on each EVM chain → BTC targets (9 combos)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "live: requires a running Lendaswap server (default http://localhost:3333)"]
async fn matrix_usdt_on_evm_chains_to_btc_targets() {
    init_tracing();
    let client = client();
    for source_chain in evm_chains() {
        let usdt = well_known::usdt(source_chain.clone())
            .unwrap_or_else(|| panic!("expected known USDT on {source_chain:?}"));
        for target in btc_chains() {
            let req = make_reverse_request(
                Chain::Known(source_chain.clone()),
                usdt.clone(),
                Chain::Known(target.clone()),
            );
            let resp = client
                .get_quote(req.clone())
                .await
                .unwrap_or_else(|e| panic!("quote USDT@{source_chain:?} -> {target:?}: {e:?}"));
            assert_quote_invariants(&req, &resp);
            tracing::info!(
                ?source_chain,
                ?target,
                rate = %resp.exchange_rate,
                protocol_fee = resp.protocol_fee,
                net_target = %resp.net_target_amount,
                "USDT@{source_chain:?} -> BTC@{target:?}",
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Matrix 5: Lightning ↔ Arkade (2 combos)
//
// Currently the only BTC-rail cross-pair the backend supports. Bitcoin ↔
// Lightning and Bitcoin ↔ Arkade are not quotable today; if that changes,
// extend the `pairs` array.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "live: requires a running Lendaswap server (default http://localhost:3333)"]
async fn matrix_lightning_arkade_cross_quotes() {
    init_tracing();
    let client = client();
    let pairs = [
        (KnownChain::Lightning, KnownChain::Arkade),
        (KnownChain::Arkade, KnownChain::Lightning),
    ];
    for (source, target) in pairs {
        let req = make_request(
            Chain::Known(source.clone()),
            Chain::Known(target.clone()),
            TokenId::btc(),
        );
        let resp = client
            .get_quote(req.clone())
            .await
            .unwrap_or_else(|e| panic!("quote BTC@{source:?} -> BTC@{target:?}: {e:?}"));
        assert_quote_invariants(&req, &resp);
        tracing::info!(
            ?source,
            ?target,
            rate = %resp.exchange_rate,
            protocol_fee = resp.protocol_fee,
            net_target = %resp.net_target_amount,
            "BTC@{source:?} -> BTC@{target:?}",
        );
    }
}
