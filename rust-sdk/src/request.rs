//! Endpoint description trait, the chokepoint used by [`crate::Client::send`].
//!
//! Centralising request dispatch through a single trait gives us one place to
//! plug in cross-cutting concerns (auth headers, retry, tracing, structured
//! logging) when we need them. The public client methods like
//! [`crate::Client::get_quote`] keep their friendly, monomorphic signatures so
//! the FFI shim stays trivial — generics live only on the internal trait.

use reqwest::Method;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Where the request's data lives on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadKind {
    /// Serialise the request into the URL's query string (typical for GETs).
    Query,
    /// Serialise the request as the JSON body (typical for POSTs / PUTs).
    JsonBody,
    /// The request carries no data — body and query string are empty.
    None,
}

/// A typed API endpoint description.
///
/// Implementors declare HTTP method, path, payload location, and response
/// type; [`crate::Client::send`] does the rest.
pub trait Endpoint: Serialize {
    type Response: DeserializeOwned;

    const METHOD: Method;
    const PATH: &'static str;
    const PAYLOAD: PayloadKind;
}
