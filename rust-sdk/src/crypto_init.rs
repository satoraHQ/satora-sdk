//! Idempotent rustls CryptoProvider install.
//!
//! Background: rustls 0.23 made the crypto backend explicit — the
//! first TLS handshake panics with
//!
//!     "Could not automatically determine the process-level
//!      CryptoProvider from Rustls crate features."
//!
//! unless something has called
//! [`rustls::crypto::CryptoProvider::install_default`] earlier. The
//! SDK touches TLS through several transitive paths (Arkade gRPC via
//! `tonic`/`ark-rs`, esplora over HTTPS, alloy's bundler/node calls),
//! so we install the provider at every public `Client` entry point.
//!
//! Why `ring` and not `aws-lc-rs`: `ring` builds without a C
//! toolchain and works cleanly across all our target triples (incl.
//! cross-compiled linux-arm64 / osx-x64 in CI). `aws-lc-rs` produces
//! smaller hot-path crypto but needs CMake/clang. Easy to flip later
//! by swapping the Cargo feature + this module's `default_provider`
//! call.

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
