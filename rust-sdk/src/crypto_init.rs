//! Idempotent rustls CryptoProvider install.
//!
//! Background: rustls 0.23 made the crypto backend explicit — the
//! first TLS handshake panics with
//!
//! ```text
//! "Could not automatically determine the process-level
//!  CryptoProvider from Rustls crate features."
//! ```
//!
//! unless something has called
//! [`rustls::crypto::CryptoProvider::install_default`] earlier. The
//! SDK touches TLS through several transitive paths (Arkade gRPC via
//! `tonic`/`ark-rs`, esplora over HTTPS, alloy's bundler/node calls),
//! so we install the provider at every public `Client` entry point.
//!
//! The SDK selects ring for its process default and respects a provider
//! already installed by the application.

use std::sync::Once;

static INIT: Once = Once::new();

/// Install rustls' `ring` crypto provider as the process-wide
/// default. Cheap to call repeatedly (`Once` short-circuits). Tolerates
/// the host already having installed a provider — `install_default`
/// returns `Err(existing)` in that case and we just drop the result.
pub(crate) fn ensure_default_provider_installed() {
    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Construct an HTTP client with the selected SDK TLS backend, even if Cargo
/// feature unification has enabled a different Reqwest default.
pub(crate) fn http_builder() -> reqwest::ClientBuilder {
    ensure_default_provider_installed();
    let builder = reqwest::Client::builder();
    #[cfg(feature = "native-tls")]
    let builder = builder.tls_backend_native();
    #[cfg(all(feature = "rustls-tls", not(feature = "native-tls")))]
    let builder = builder.tls_backend_rustls();
    builder
}
