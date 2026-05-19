//! Lendaswap Client SDK.
//!
//! Hand-written, FFI-friendly Rust client for the Lendaswap HTTP API. The
//! request and response types defined here are validated against the upstream
//! `openapi.json` in the integration tests.

pub mod client;
pub mod error;
pub mod request;
pub mod signer;
pub mod storage;
pub mod types;

// Account-abstraction: gasless EVM funding via ERC-4337 + EIP-7702.
// Feature-gated so the base SDK doesn't pull in alloy.
#[cfg(feature = "gasless")]
pub mod aa;

// Arkade VHTLC claim flow (offchain spend). Feature-gated so the base
// SDK doesn't pull in ark-rs + gRPC + esplora-client. Owns the
// boilerplate `ark-client` requires (wallet, blockchain, persistence
// impls) so consumers don't have to re-implement them.
#[cfg(feature = "arkade-claim")]
pub mod arkade;

// Internal-only: wire-format structs that public types route through via
// `#[serde(into = …)] / from = …`. Crate-private so downstream callers can
// never accidentally depend on them.
mod wire;

pub use client::Client;
pub use client::ClientBuilder;
pub use client::Swap;
pub use client::SwapFunding;
pub use error::Error;
pub use error::Result;
pub use request::Endpoint;
pub use request::PayloadKind;
pub use signer::Signer;
pub use storage::InMemorySwapStorage;
pub use storage::SwapStorage;
