//! Lendaswap Client SDK.
//!
//! Hand-written, FFI-friendly Rust client for the Lendaswap HTTP API. The
//! request and response types defined here are validated against the upstream
//! `openapi.json` in the integration tests.

pub mod client;
pub mod error;
pub mod request;
pub mod types;

// Internal-only: wire-format structs that public types route through via
// `#[serde(into = …)] / from = …`. Crate-private so downstream callers can
// never accidentally depend on them.
mod wire;

pub use client::Client;
pub use error::Error;
pub use error::Result;
pub use request::Endpoint;
pub use request::PayloadKind;
