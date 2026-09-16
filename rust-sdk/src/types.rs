//! API request and response types.
//!
//! Each public type below mirrors a named schema in the upstream `openapi.json`.
//! The `tests/openapi_schema.rs` integration test serializes representative
//! values of these types and validates them against the spec to catch drift.

mod address;
mod chain;
mod quote;
mod swap;
mod token;
mod version;

pub use address::Address;
pub use chain::Chain;
pub use chain::KnownChain;
pub use quote::QuoteAmount;
pub use quote::QuoteRequest;
pub use quote::QuoteResponse;
use serde::Deserialize;
use serde::Serialize;
pub use swap::ArkadeToLightningSwapResponse;
pub use swap::CreateArkadeToLightningSwapRequest;
pub use swap::CreateEvmToArkadeSwapRequest;
pub use swap::CreateLightningToArkadeSwapRequest;
pub use swap::EvmToArkadeSwapResponse;
pub use swap::GetSwapResponse;
pub use swap::LightningDestination;
pub use swap::LightningToArkadeSwapResponse;
pub use swap::SwapStatus;
pub use swap::TokenInfo;
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

mod discovery;
mod responses;
pub use discovery::BulkStatusResponse;
pub use discovery::ChainConfigEntry;
pub use discovery::ChainConfigResponse;
pub use discovery::LightningSendQuoteResponse;
pub use discovery::MtpResponse;
pub use discovery::NetworkFee;
pub use discovery::NetworkFeePairEntry;
pub use discovery::NetworkFeesResponse;
pub use discovery::ReferralFeeResponse;
pub use discovery::ServiceStatus;
pub use discovery::ServicesStatus;
pub use discovery::StatusResponse;
pub use discovery::SupportAgentInfo;
pub use discovery::SupportAgentsResponse;
pub use discovery::SwapPairInfo;
pub use discovery::SwapPairsResponse;
pub use discovery::SwapStatusEntry;
pub use discovery::TokenInfos;
pub use discovery::TokenRef;
pub use responses::ArkadeToEvmSwapResponse;
pub use responses::BitcoinToEvmSwapResponse;
pub use responses::BtcToArkadeSwapResponse;
pub use responses::EvmToBitcoinSwapResponse;
pub use responses::EvmToLightningSwapResponse;
pub use responses::LightningToEvmSwapResponse;
mod recovery;
pub use recovery::RecoveredSwap;
pub use recovery::RecoveryResponse;

mod creation;
pub use creation::BridgeRoute;
pub use creation::EvmAsset;
pub use creation::SwapCreation;
pub use creation::SwapOptions;
