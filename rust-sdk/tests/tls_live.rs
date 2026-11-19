#![cfg(all(
    any(feature = "rustls-tls", feature = "native-tls"),
    feature = "websocket",
    not(target_arch = "wasm32")
))]

use lendaswap_sdk::Client;
use lendaswap_sdk::subscription::StatusEvent;
use lendaswap_sdk::subscription::SubscriptionOptions;
use std::time::Duration;

/// Exercises the default clients and certificate validation against the live API.
#[tokio::test]
#[ignore = "requires network access to the Satora API"]
async fn connects_over_https_and_wss() {
    let url =
        std::env::var("SATORA_TLS_TEST_URL").unwrap_or_else(|_| "https://api.satora.io".into());
    assert!(url.starts_with("https://"));
    let client = Client::new(&url).unwrap();
    assert_eq!(client.health().await.unwrap().trim(), "OK");
    let mut statuses = client
        .subscribe_status(SubscriptionOptions::default())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match statuses.recv().await {
                Some(StatusEvent::Connected { .. }) => break,
                Some(StatusEvent::Error { message }) => panic!("{message}"),
                Some(_) => continue,
                None => panic!("status subscription closed"),
            }
        }
    })
    .await
    .expect("WebSocket handshake timed out");
    statuses.shutdown().await.unwrap();
}
