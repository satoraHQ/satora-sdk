use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// All errors surfaced by the SDK.
///
/// Variants are plain owned data so this enum can later be projected across
/// an FFI boundary without lifetime gymnastics.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("operation cancelled")]
    Cancelled,

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

    /// Client-side quote composition does not cover this route; the server's
    /// `/quote` may still price it.
    #[error("quote path not supported by composition: {0}")]
    UnsupportedQuotePath(String),

    #[error("operation timed out: {0}")]
    Timeout(String),

    #[error("swap storage error: {0}")]
    Storage(String),

    /// An earlier funding of the swap may have gone out, and the SDK cannot tell.
    /// Never fund the swap by other means and do not treat it as unfunded: follow its
    /// status meanwhile, and give it up only once a check by hand shows where the funds
    /// went.
    ///
    /// From [`Client::fund_arkade_swap`](crate::Client::fund_arkade_swap), a spender of
    /// a journaled input that arkd names resolves on a later call, once arkd shows its
    /// VHTLC output or returns it paying nothing to the VHTLC. A spender arkd does not
    /// name, a journaled input arkd does not know, and a journal for another VHTLC
    /// address never resolve on their own.
    ///
    /// From the EVM funding methods, a funding reserved without a recorded hash resolves
    /// once the caller attaches its transaction with `attach_evm_funding_transaction`,
    /// or releases `evm_fund_started` after making sure nothing was sent. A `cctp_fund`
    /// batch without one resolves once the caller attaches its transaction with
    /// `attach_bridge_transaction`, and a relay funding signature once
    /// `reconcile_gasless_relay_funding` reports it funded or expired. A funding whose
    /// hash is recorded is refused with [`Error::InvalidSwap`] instead, as its receipt
    /// tells the outcome.
    ///
    /// From `recover_evm_deposit_with_wallet` and `sweep_evm_deposit`, a relay funding
    /// signature can still pull the deposit, and nothing was sent. It resolves on its
    /// own once `reconcile_gasless_relay_funding` reports the relay funding funded or
    /// expired, or once chain time passes the signatures' deadline by 120 seconds.
    #[error("unresolved funding: {0}")]
    UnresolvedFunding(String),

    /// The wallet holds less native gas than the transaction's estimated fee.
    /// Nothing was reserved or sent: send the wallet gas and call again.
    #[error("insufficient native gas: {0}")]
    InsufficientGas(String),
}

impl From<rusqlite::Error> for Error {
    fn from(err: rusqlite::Error) -> Self {
        Self::Storage(err.to_string())
    }
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
