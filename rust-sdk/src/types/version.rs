//! `GET /version` types.

use crate::request::Endpoint;
use crate::request::PayloadKind;
use reqwest::Method;
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

/// Marker request for `GET /version` — empty payload, no query, no body.
/// Private; reached only through [`crate::Client::version`].
#[derive(Serialize)]
pub(crate) struct VersionRequest;

impl Endpoint for VersionRequest {
    type Response = Version;
    const METHOD: Method = Method::GET;
    const PATH: &'static str = "version";
    const PAYLOAD: PayloadKind = PayloadKind::None;
}
