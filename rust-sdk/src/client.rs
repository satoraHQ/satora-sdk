//! HTTP client for the Lendaswap API.
//!
//! The public surface intentionally avoids generics and borrowed inputs so the
//! same shape can be re-exposed over a C-ABI / `csbindgen` / `interoptopus`
//! layer in a future crate.

use crate::error::Error;
use crate::error::Result;
use crate::types::ErrorResponse;
use crate::types::Version;
use reqwest::StatusCode;
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

    /// `GET /health` — returns the raw `text/plain` body.
    pub async fn health(&self) -> Result<String> {
        let url = self.url("health")?;
        let resp = self.http.get(url).send().await?;
        let resp = check_status(resp).await?;
        Ok(resp.text().await?)
    }

    /// `GET /version`.
    pub async fn version(&self) -> Result<Version> {
        self.get_json("version").await
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.url(path)?;
        let resp = self.http.get(url).send().await?;
        let resp = check_status(resp).await?;
        Ok(resp.json::<T>().await?)
    }

    fn url(&self, path: &str) -> Result<Url> {
        // join() respects whether `base_url` ends in a slash; we always treat
        // `path` as a relative segment with no leading slash.
        Ok(self.base_url.join(&format!("/{path}"))?)
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
