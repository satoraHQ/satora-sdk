//! API request and response types.
//!
//! Each type below mirrors a named schema in the upstream `openapi.json`. The
//! `tests/openapi_schema.rs` integration test serializes representative values
//! of these types and validates them against the spec to catch drift.

use serde::Deserialize;
use serde::Serialize;

/// Response of `GET /version`.
///
/// Maps to the `Version` component schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Version {
    pub tag: String,
    pub commit_hash: String,
}

/// Standard error body returned by the API on non-2xx responses.
///
/// Maps to the `ErrorResponse` component schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorResponse {
    pub error: String,
}
