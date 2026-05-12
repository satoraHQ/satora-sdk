use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// All errors surfaced by the SDK.
///
/// Variants are plain owned data so this enum can later be projected across
/// an FFI boundary without lifetime gymnastics.
#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid base URL: {0}")]
    InvalidBaseUrl(String),

    #[error("HTTP transport error: {0}")]
    Transport(String),

    #[error("failed to decode response body: {0}")]
    Decode(String),

    #[error("API returned HTTP {status}: {message}")]
    Api { status: u16, message: String },

    #[error("invalid signer: {0}")]
    InvalidSigner(String),

    #[error("invalid swap arguments: {0}")]
    InvalidSwap(String),
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        if err.is_decode() {
            Error::Decode(err.to_string())
        } else {
            Error::Transport(err.to_string())
        }
    }
}

impl From<url::ParseError> for Error {
    fn from(err: url::ParseError) -> Self {
        Error::InvalidBaseUrl(err.to_string())
    }
}
