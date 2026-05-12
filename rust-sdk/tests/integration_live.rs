//! Live integration tests against a running Lendaswap server.
//!
//! Disabled by default (`#[ignore]`) so `cargo test` stays hermetic. Run
//! manually with:
//!
//! ```bash
//! # Defaults to http://localhost:3333
//! cargo test --test integration_live -- --ignored
//!
//! # Or point at a different deployment:
//! LENDASWAP_API_URL=https://api.satora.io cargo test --test integration_live -- --ignored
//! ```

use lendaswap_sdk::Client;

const DEFAULT_BASE_URL: &str = "http://localhost:3333";

fn base_url() -> String {
    std::env::var("LENDASWAP_API_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
}

fn client() -> Client {
    Client::new(&base_url()).expect("base URL parses")
}

#[tokio::test]
#[ignore = "live: requires a running Lendaswap server (default http://localhost:3333)"]
async fn live_health() {
    let body = client()
        .health()
        .await
        .expect("GET /health succeeded against live server");
    assert!(
        !body.trim().is_empty(),
        "expected non-empty /health body, got {body:?}"
    );
}

#[tokio::test]
#[ignore = "live: requires a running Lendaswap server (default http://localhost:3333)"]
async fn live_version() {
    let version = client()
        .version()
        .await
        .expect("GET /version succeeded against live server");
    assert!(!version.tag.is_empty(), "version.tag was empty");
    assert!(
        !version.commit_hash.is_empty(),
        "version.commit_hash was empty"
    );
}
