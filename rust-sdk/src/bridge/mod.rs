//! CCTP v2 and USDT0 transfers, attestation validation, and resumable execution.
pub mod cctp;
mod execution;
pub mod usdt0;
use crate::Error;
use crate::Result;
pub use execution::BridgeRecovery;
pub use execution::BridgeSubmission;
pub use execution::CctpDestination;
pub use execution::CctpMint;
use reqwest::Client;
use reqwest::Response;
use serde::de::DeserializeOwned;
use url::Url;

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidSwap(message.into())
}

#[derive(Clone, Debug)]
pub struct BridgeApi {
    http: Client,
    iris: Url,
    layerzero: Url,
}
impl BridgeApi {
    pub fn new(iris: &str, layerzero: &str) -> Result<Self> {
        Ok(Self {
            http: Client::new(),
            iris: Url::parse(iris)?,
            layerzero: Url::parse(layerzero)?,
        })
    }
    pub fn mainnet() -> Result<Self> {
        Self::new(
            "https://iris-api.circle.com",
            "https://scan.layerzero-api.com",
        )
    }
}
async fn pending_json<T: DeserializeOwned>(response: Response) -> Result<Option<T>> {
    let status = response.status();
    if status.as_u16() == 404 || status.as_u16() == 429 || status.is_server_error() {
        return Ok(None);
    }
    Ok(Some(
        crate::client::check_status(response).await?.json().await?,
    ))
}

#[cfg(test)]
mod execution_tests;
#[cfg(test)]
mod tests;
