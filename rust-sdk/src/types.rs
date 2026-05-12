//! API request and response types.
//!
//! Each public type below mirrors a named schema in the upstream `openapi.json`.
//! The `tests/openapi_schema.rs` integration test serializes representative
//! values of these types and validates them against the spec to catch drift.

mod chain;
mod quote;
mod token;
mod version;

pub use chain::Chain;
pub use chain::KnownChain;
pub use quote::QuoteAmount;
pub use quote::QuoteRequest;
pub use quote::QuoteResponse;
use serde::Deserialize;
use serde::Serialize;
pub use token::TokenId;
pub use version::Version;
pub(crate) use version::VersionRequest;

/// Standard error body returned by the API on non-2xx responses.
///
/// Maps to the `ErrorResponse` component schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorResponse {
    pub error: String,
}
