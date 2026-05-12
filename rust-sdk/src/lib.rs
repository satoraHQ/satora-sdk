//! Lendaswap Client SDK.
//!
//! Hand-written, FFI-friendly Rust client for the Lendaswap HTTP API. The
//! request and response types defined here are validated against the upstream
//! `openapi.json` in the integration tests.

pub mod client;
pub mod error;
pub mod types;

pub use client::Client;
pub use error::Error;
pub use error::Result;
