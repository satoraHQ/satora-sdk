//! UniFFI bindings to `lendaswap-sdk`.
//!
//! Every public function here is `extern` over C ABI (via uniffi's
//! scaffolding) and synchronous. Internally we run a single multi-thread
//! tokio runtime and `block_on` each async SDK call — C# / .NET callers
//! get plain blocking functions, which they typically wrap in
//! `Task.Run(...)` if they need async-on-the-managed-side.
//!
//! Why blocking and not uniffi's async feature? Two reasons:
//!
//! 1. Async UniFFI exports require the foreign language to drive a poll loop (in C#, that's still
//!    community-supported as of `uniffi-bindgen-cs` 0.10). Sync is the lowest-risk path today.
//! 2. btcpayserver-style consumers already block on I/O inside request handlers; wrapping with
//!    `Task.Run` is one line per call and keeps the FFI surface trivial.
//!
//! When uniffi's async C# story matures we can flip individual exports
//! to `async fn` without touching call sites that take `Result<T, E>`.

use lendaswap_sdk::Client;
use lendaswap_sdk::Error as SdkErrorInner;
use lendaswap_sdk::types::Chain as SdkChain;
use lendaswap_sdk::types::KnownChain as SdkKnownChain;
use lendaswap_sdk::types::QuoteAmount as SdkQuoteAmount;
use lendaswap_sdk::types::QuoteRequest;
use lendaswap_sdk::types::TokenId as SdkTokenId;
use std::sync::OnceLock;

uniffi::setup_scaffolding!();

/// Single shared runtime for the lifetime of the loaded shared library.
/// Multi-thread so concurrent FFI calls don't queue on one worker.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("lendaswap-sdk-ffi")
            .build()
            .expect("failed to build tokio runtime for lendaswap-sdk-ffi")
    })
}

/// Version reported by the backend (`GET /version`). Mirrors
/// `lendaswap_sdk::types::Version` — copied here as a `uniffi::Record`
/// so the FFI surface owns its own data shape and can evolve
/// independently of the Rust type.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub tag: String,
    pub commit_hash: String,
}

/// FFI-visible error variants. Flattens the Rust `Error` enum to one
/// `Internal { message }` variant for now — the goal is to keep the C#
/// surface tiny while we settle on which errors callers actually need
/// to discriminate. We can split out specific variants
/// (`InvalidBaseUrl`, `Transport`, `Api { status }`) later without
/// breaking match exhaustiveness on the C# side, which has to handle
/// the catch-all anyway.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum SdkError {
    #[error("SDK error: {message}")]
    Internal { message: String },
}

impl From<SdkErrorInner> for SdkError {
    fn from(e: SdkErrorInner) -> Self {
        Self::Internal {
            message: e.to_string(),
        }
    }
}

/// Fetch the deployed Lendaswap backend's version + commit hash.
///
/// Canary endpoint for the FFI surface — exercises the full build
/// chain (Rust → cdylib → uniffi-generated C# → managed test) with
/// minimal domain logic. If this works end-to-end, every other method
/// is mechanical.
#[uniffi::export]
pub fn fetch_version(base_url: String) -> Result<Version, SdkError> {
    runtime().block_on(async {
        let client = Client::new(&base_url)?;
        let v = client.version().await?;
        Ok(Version {
            tag: v.tag,
            commit_hash: v.commit_hash,
        })
    })
}

/// Compact view of the quote endpoint's response. Mirrors
/// `lendaswap_sdk::types::QuoteResponse` minus the wire-only fields
/// that aren't useful to a non-Rust caller.
///
/// Large monetary quantities stay as strings (matching the backend's
/// wire format) to side-step JS-style number-precision issues and let
/// callers parse with whatever big-int library they prefer.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct QuoteResult {
    pub exchange_rate: String,
    pub network_fee: u64,
    pub gasless_network_fee: u64,
    pub protocol_fee: u64,
    pub protocol_fee_rate: f64,
    pub min_amount: u64,
    pub max_amount: u64,
    pub source_amount: String,
    pub target_amount: String,
    pub net_source_amount: String,
    pub net_target_amount: String,
    pub bridge_fee: Option<u64>,
}

/// Chain identifiers exposed across FFI. Mirrors
/// `lendaswap_sdk::types::Chain` — the underlying SDK type can't carry
/// `#[derive(uniffi::Enum)]` itself (uniffi only derives on types in
/// this crate), so we redefine here and bridge via `From`.
///
/// `Other` is the forward-compat escape hatch: any wire value the SDK
/// doesn't recognise round-trips through it without losing fidelity.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ChainId {
    Polygon,
    Ethereum,
    Arbitrum,
    Arkade,
    Lightning,
    Bitcoin,
    Other { wire: String },
}

impl From<ChainId> for SdkChain {
    fn from(c: ChainId) -> Self {
        match c {
            ChainId::Polygon => SdkChain::Known(SdkKnownChain::Polygon),
            ChainId::Ethereum => SdkChain::Known(SdkKnownChain::Ethereum),
            ChainId::Arbitrum => SdkChain::Known(SdkKnownChain::Arbitrum),
            ChainId::Arkade => SdkChain::Known(SdkKnownChain::Arkade),
            ChainId::Lightning => SdkChain::Known(SdkKnownChain::Lightning),
            ChainId::Bitcoin => SdkChain::Known(SdkKnownChain::Bitcoin),
            ChainId::Other { wire } => SdkChain::Other(wire),
        }
    }
}

/// Token identifiers exposed across FFI. Mirrors
/// `lendaswap_sdk::types::TokenId`. `Other { wire }` carries any
/// contract address or wire string the SDK doesn't name.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TokenId {
    Btc,
    UsdcPolygon,
    UsdcArbitrum,
    UsdcEthereum,
    UsdtPolygon,
    UsdtEthereum,
    Usdt0Arbitrum,
    WbtcPolygon,
    WbtcArbitrum,
    WbtcEthereum,
    Other { wire: String },
}

impl From<TokenId> for SdkTokenId {
    fn from(t: TokenId) -> Self {
        match t {
            TokenId::Btc => SdkTokenId::Btc,
            TokenId::UsdcPolygon => SdkTokenId::UsdcPolygon,
            TokenId::UsdcArbitrum => SdkTokenId::UsdcArbitrum,
            TokenId::UsdcEthereum => SdkTokenId::UsdcEthereum,
            TokenId::UsdtPolygon => SdkTokenId::UsdtPolygon,
            TokenId::UsdtEthereum => SdkTokenId::UsdtEthereum,
            TokenId::Usdt0Arbitrum => SdkTokenId::Usdt0Arbitrum,
            TokenId::WbtcPolygon => SdkTokenId::WbtcPolygon,
            TokenId::WbtcArbitrum => SdkTokenId::WbtcArbitrum,
            TokenId::WbtcEthereum => SdkTokenId::WbtcEthereum,
            TokenId::Other { wire } => SdkTokenId::Other(wire),
        }
    }
}

/// Source / target amount mutex. Mirrors `lendaswap_sdk::types::QuoteAmount`
/// — encoding the invariant at the type level means the FFI surface no
/// longer needs the "exactly one of two Options" runtime check.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq, Hash)]
pub enum QuoteAmount {
    /// Amount in the smallest unit of the source token.
    Source { units: u64 },
    /// Amount in the smallest unit of the target token.
    Target { units: u64 },
}

impl From<QuoteAmount> for SdkQuoteAmount {
    fn from(a: QuoteAmount) -> Self {
        match a {
            QuoteAmount::Source { units } => SdkQuoteAmount::Source(units),
            QuoteAmount::Target { units } => SdkQuoteAmount::Target(units),
        }
    }
}

/// Fetch a swap quote. Chain / token / amount are typed enums; the
/// "exactly one of source/target" invariant is enforced by the
/// `QuoteAmount` discriminator instead of runtime validation.
#[uniffi::export]
pub fn fetch_quote(
    base_url: String,
    source_chain: ChainId,
    source_token: TokenId,
    target_chain: ChainId,
    target_token: TokenId,
    amount: QuoteAmount,
) -> Result<QuoteResult, SdkError> {
    runtime().block_on(async {
        let client = Client::new(&base_url)?;
        let req = QuoteRequest::new(
            source_chain.into(),
            source_token.into(),
            target_chain.into(),
            target_token.into(),
            amount.into(),
        );
        let resp = client.get_quote(req).await?;
        Ok(QuoteResult {
            exchange_rate: resp.exchange_rate,
            network_fee: resp.network_fee,
            gasless_network_fee: resp.gasless_network_fee,
            protocol_fee: resp.protocol_fee,
            protocol_fee_rate: resp.protocol_fee_rate,
            min_amount: resp.min_amount,
            max_amount: resp.max_amount,
            source_amount: resp.source_amount,
            target_amount: resp.target_amount,
            net_source_amount: resp.net_source_amount,
            net_target_amount: resp.net_target_amount,
            bridge_fee: resp.bridge_fee,
        })
    })
}
