//! HTTP client for the Lendaswap API.
//!
//! The public surface intentionally avoids generics and borrowed inputs so the
//! same shape can be re-exposed over a C-ABI / `csbindgen` / `interoptopus`
//! layer in a future crate. Generics only appear on the internal
//! [`Client::send`] helper, which is the single chokepoint every endpoint
//! flows through — that's where auth headers, retry, and tracing will plug in.

use crate::error::Error;
use crate::error::Result;
use crate::request::Endpoint;
use crate::request::PayloadKind;
use crate::types::ErrorResponse;
use crate::types::QuoteRequest;
use crate::types::QuoteResponse;
use crate::types::Version;
use crate::types::VersionRequest;
use reqwest::StatusCode;
use serde::Serialize;
use url::Url;

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: Url,
}

impl Client {
    /// Construct a new client targeting `base_url` (e.g. `https://api.lendaswap.com`).
    pub fn new(base_url: &str) -> Result<Self> {
        let base_url = Url::parse(base_url)?;
        Ok(Self {
            http: reqwest::Client::new(),
            base_url,
        })
    }

    /// Construct a client from an existing `reqwest::Client`. Useful for
    /// injecting middleware, custom timeouts, or shared connection pools.
    pub fn with_http(base_url: &str, http: reqwest::Client) -> Result<Self> {
        let base_url = Url::parse(base_url)?;
        Ok(Self { http, base_url })
    }

    /// Internal dispatch chokepoint for every endpoint described by
    /// [`Endpoint`]. Builds the URL, attaches the payload (query / body /
    /// none), sends, maps non-2xx responses to [`Error::Api`], and decodes
    /// JSON into the endpoint's `Response` type.
    #[tracing::instrument(
        name = "send",
        skip_all,
        fields(method = %E::METHOD, path = E::PATH),
    )]
    pub async fn send<E: Endpoint>(&self, req: E) -> Result<E::Response> {
        let url = self.url(E::PATH)?;
        tracing::debug!(%url, "sending request");
        let builder = self.http.request(E::METHOD, url);
        let builder = attach_payload(builder, &req, E::PAYLOAD);
        let resp = builder.send().await?;
        tracing::debug!(status = %resp.status(), "response received");
        let resp = check_status(resp).await?;
        Ok(resp.json::<E::Response>().await?)
    }

    /// `GET /health` — returns the raw `text/plain` body.
    ///
    /// Not routed through [`Self::send`] because the response is plain text
    /// rather than JSON.
    #[tracing::instrument(name = "health", skip_all)]
    pub async fn health(&self) -> Result<String> {
        let url = self.url("health")?;
        tracing::debug!(%url, "sending health probe");
        let resp = self.http.get(url).send().await?;
        tracing::debug!(status = %resp.status(), "response received");
        let resp = check_status(resp).await?;
        Ok(resp.text().await?)
    }

    /// `GET /version`.
    pub async fn version(&self) -> Result<Version> {
        self.send(VersionRequest).await
    }

    /// `GET /quote` — fetch an exchange quote.
    ///
    /// Exactly one of source / target amount must be specified, which the
    /// `QuoteAmount` enum enforces at the type level.
    pub async fn get_quote(&self, req: QuoteRequest) -> Result<QuoteResponse> {
        self.send(req).await
    }

    fn url(&self, path: &str) -> Result<Url> {
        // join() respects whether `base_url` ends in a slash; we always treat
        // `path` as a relative segment with no leading slash.
        Ok(self.base_url.join(&format!("/{path}"))?)
    }
}

fn attach_payload<E: Serialize>(
    builder: reqwest::RequestBuilder,
    req: &E,
    kind: PayloadKind,
) -> reqwest::RequestBuilder {
    match kind {
        PayloadKind::Query => builder.query(req),
        PayloadKind::JsonBody => builder.json(req),
        PayloadKind::None => builder,
    }
}

async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let code = status.as_u16();
    let message = match resp.json::<ErrorResponse>().await {
        Ok(body) => body.error,
        Err(_) => default_status_message(status),
    };
    tracing::warn!(status = code, %message, "API returned non-2xx");
    Err(Error::Api {
        status: code,
        message,
    })
}

fn default_status_message(status: StatusCode) -> String {
    status
        .canonical_reason()
        .map(|s| s.to_string())
        .unwrap_or_else(|| status.to_string())
}
