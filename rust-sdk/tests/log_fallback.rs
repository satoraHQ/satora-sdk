//! Its own test binary because tracing forwards to `log` only while no tracing
//! dispatcher has ever been set in the process, and other tests set one.

use lendaswap_sdk::Client;
use serde_json::json;
use std::sync::Mutex;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

struct Capture(Mutex<Vec<(log::Level, String)>>);

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        if record.target() == "lendaswap_sdk" {
            self.0
                .lock()
                .unwrap()
                .push((record.level(), record.args().to_string()));
        }
    }

    fn flush(&self) {}
}

static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));

#[tokio::test]
async fn default_sink_reaches_a_log_logger_without_a_tracing_subscriber() {
    log::set_logger(&CAPTURE).unwrap();
    log::set_max_level(log::LevelFilter::Trace);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/swap/recover"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "swaps": [], "highest_index": 0, "next_index": 0, "scanned_until": 9, "complete": true
        })))
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .mnemonic(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .build()
        .unwrap();
    client.recover_swaps(None).await.unwrap();
    let captured = CAPTURE.0.lock().unwrap().clone();
    assert_eq!(captured.len(), 2, "{captured:?}");
    assert_eq!(captured[0].0, log::Level::Info);
    assert!(
        captured[0].1.starts_with("Recovering swaps for wallet")
            && captured[0].1.contains(r#"event="client.recover.start""#),
        "{captured:?}"
    );
    assert_eq!(captured[1].0, log::Level::Debug);
    assert!(
        captured[1].1.contains(r#"event="client.recover.response""#),
        "{captured:?}"
    );
}
