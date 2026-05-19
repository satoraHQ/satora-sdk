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
use lendaswap_sdk::Swap as SdkSwap;
use lendaswap_sdk::SwapFunding as SdkSwapFunding;
use lendaswap_sdk::types::Address as SdkAddress;
use lendaswap_sdk::types::Chain as SdkChain;
use lendaswap_sdk::types::KnownChain as SdkKnownChain;
use lendaswap_sdk::types::QuoteAmount as SdkQuoteAmount;
use lendaswap_sdk::types::QuoteRequest;
use lendaswap_sdk::types::SwapStatus as SdkSwapStatus;
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

/// Stateful FFI client — wraps one `lendaswap_sdk::Client` for its
/// lifetime so storage (swap_id → key_index) persists across calls.
/// Without that continuity, a `create_swap`/`fund_swap`/`claim` chain
/// from foreign code couldn't recover the per-swap secret material.
///
/// Constructed via [`Self::new`] (read-only, supports version + quote
/// only) or [`Self::new_signing`] (full surface — required for
/// `create_swap`, funding, and claim). Both forms use the default
/// in-memory swap storage; the FFI doesn't expose a way to plug in a
/// custom backend yet.
#[derive(uniffi::Object)]
pub struct LendaswapClient {
    inner: Client,
}

#[uniffi::export]
impl LendaswapClient {
    /// Read-only client. Supports [`Self::version`] and
    /// [`Self::quote`]; any signer-requiring method (`create_swap`,
    /// `fund_swap_gasless`, `claim`) errors with `InvalidSigner` when
    /// invoked through a non-signing client.
    #[uniffi::constructor]
    pub fn new(base_url: String) -> Result<std::sync::Arc<Self>, SdkError> {
        let inner = Client::new(&base_url)?;
        Ok(std::sync::Arc::new(Self { inner }))
    }

    /// Signing client. Required for any method that derives per-swap
    /// secret material (preimage, EVM key) from the mnemonic.
    #[uniffi::constructor]
    pub fn new_signing(
        base_url: String,
        mnemonic: String,
    ) -> Result<std::sync::Arc<Self>, SdkError> {
        let inner = Client::builder()
            .base_url(&base_url)
            .mnemonic(&mnemonic)
            .build()?;
        Ok(std::sync::Arc::new(Self { inner }))
    }

    /// `GET /version` — canary endpoint. Exercises the full build
    /// chain (Rust → cdylib → uniffi-generated C# → managed test)
    /// with minimal domain logic.
    pub fn version(&self) -> Result<Version, SdkError> {
        runtime().block_on(async {
            let v = self.inner.version().await?;
            Ok(Version {
                tag: v.tag,
                commit_hash: v.commit_hash,
            })
        })
    }
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

// Quote method block — split from the constructor / version block so
// the chain/token/amount enum definitions can sit between them
// (uniffi::export only sees one impl block at a time, but rustc is
// fine with multiple).
#[uniffi::export]
impl LendaswapClient {
    /// Fetch a swap quote. Chain / token / amount are typed enums;
    /// the "exactly one of source/target" invariant is enforced by
    /// the `QuoteAmount` discriminator instead of runtime validation.
    pub fn quote(
        &self,
        source_chain: ChainId,
        source_token: TokenId,
        target_chain: ChainId,
        target_token: TokenId,
        amount: QuoteAmount,
    ) -> Result<QuoteResult, SdkError> {
        runtime().block_on(async {
            let req = QuoteRequest::new(
                source_chain.into(),
                source_token.into(),
                target_chain.into(),
                target_token.into(),
                amount.into(),
            );
            let resp = self.inner.get_quote(req).await?;
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
}

/// Receive-address tag. Mirrors `lendaswap_sdk::types::Address` — the
/// variant carries both the network and the encoded string so callers
/// can't accidentally pass a Bitcoin address where an Arkade address
/// is required (or vice versa). The SDK's direction validator catches
/// the mismatch but errors are clearer when the type system catches it
/// first.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Address {
    Arkade { address: String },
    Bitcoin { address: String },
    Lightning { invoice: String },
    Evm { address: String },
}

impl From<Address> for SdkAddress {
    fn from(a: Address) -> Self {
        match a {
            Address::Arkade { address } => SdkAddress::Arkade(address),
            Address::Bitcoin { address } => SdkAddress::Bitcoin(address),
            Address::Lightning { invoice } => SdkAddress::Lightning(invoice),
            Address::Evm { address } => SdkAddress::Evm(address),
        }
    }
}

/// State machine for a swap. Mirrors `lendaswap_sdk::types::SwapStatus`
/// 1:1 (incl. the `Other { wire }` forward-compat escape hatch) so
/// FFI callers can pattern-match on the same set of states the Rust
/// SDK exposes.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SwapStatus {
    Pending,
    ClientFundingSeen,
    ClientFunded,
    ClientRefunded,
    ServerFunded,
    ClientRedeeming,
    ClientRedeemed,
    ServerRedeemed,
    ClientFundedServerRefunded,
    ClientRefundedServerFunded,
    ClientRefundedServerRefunded,
    Expired,
    ClientInvalidFunded,
    ClientFundedTooLate,
    ServerWontFund,
    ClientRedeemedAndClientRefunded,
    /// Unrecognised wire value, preserved verbatim.
    Other {
        wire: String,
    },
}

impl From<SdkSwapStatus> for SwapStatus {
    fn from(s: SdkSwapStatus) -> Self {
        match s {
            SdkSwapStatus::Pending => Self::Pending,
            SdkSwapStatus::ClientFundingSeen => Self::ClientFundingSeen,
            SdkSwapStatus::ClientFunded => Self::ClientFunded,
            SdkSwapStatus::ClientRefunded => Self::ClientRefunded,
            SdkSwapStatus::ServerFunded => Self::ServerFunded,
            SdkSwapStatus::ClientRedeeming => Self::ClientRedeeming,
            SdkSwapStatus::ClientRedeemed => Self::ClientRedeemed,
            SdkSwapStatus::ServerRedeemed => Self::ServerRedeemed,
            SdkSwapStatus::ClientFundedServerRefunded => Self::ClientFundedServerRefunded,
            SdkSwapStatus::ClientRefundedServerFunded => Self::ClientRefundedServerFunded,
            SdkSwapStatus::ClientRefundedServerRefunded => Self::ClientRefundedServerRefunded,
            SdkSwapStatus::Expired => Self::Expired,
            SdkSwapStatus::ClientInvalidFunded => Self::ClientInvalidFunded,
            SdkSwapStatus::ClientFundedTooLate => Self::ClientFundedTooLate,
            SdkSwapStatus::ServerWontFund => Self::ServerWontFund,
            SdkSwapStatus::ClientRedeemedAndClientRefunded => Self::ClientRedeemedAndClientRefunded,
            SdkSwapStatus::Other(s) => Self::Other { wire: s },
            // `SdkSwapStatus` is `#[non_exhaustive]` upstream — keep
            // this catch-all so adding a variant there doesn't break
            // the FFI build.
            other => Self::Other {
                wire: other.as_wire_str().to_string(),
            },
        }
    }
}

/// How the user has to fund the swap. Mirrors `lendaswap_sdk::SwapFunding`.
///
/// `Gasless` carries the depositor EOA the user sends source-token to;
/// the SDK relays into the HTLC via a Permit2-signed userOp.
/// `UserSubmitted` is the non-gasless path — caller fetches HTLC calldata
/// out-of-band and broadcasts the funding tx themselves.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SwapFunding {
    Gasless { deposit_address: String },
    UserSubmitted,
}

impl From<SdkSwapFunding> for SwapFunding {
    fn from(f: SdkSwapFunding) -> Self {
        match f {
            SdkSwapFunding::Gasless { deposit_address } => Self::Gasless { deposit_address },
            SdkSwapFunding::UserSubmitted { .. } => Self::UserSubmitted,
            // `SdkSwapFunding` is `#[non_exhaustive]` upstream. Mapping
            // an unknown variant to `UserSubmitted` is wrong, but the
            // closed-set today makes the arm unreachable; keep the
            // catch-all so the FFI build doesn't break when a new
            // variant lands.
            other => panic!("unhandled SwapFunding variant: {other:?}"),
        }
    }
}

/// Compact, user-facing view of a created swap. Mirrors
/// `lendaswap_sdk::Swap` — amounts stay as strings to preserve
/// precision for large EVM token amounts.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Swap {
    pub id: String,
    pub status: SwapStatus,
    pub funding: SwapFunding,
    pub deposit_amount: String,
    pub deposit_token: TokenId,
    pub receive_address: String,
    pub receive_amount: String,
    pub receive_token: TokenId,
}

impl From<SdkSwap> for Swap {
    fn from(s: SdkSwap) -> Self {
        Self {
            id: s.id,
            status: s.status.into(),
            funding: s.funding.into(),
            deposit_amount: s.deposit_amount,
            deposit_token: token_id_from_sdk(s.deposit_token),
            receive_address: s.receive_address,
            receive_amount: s.receive_amount,
            receive_token: token_id_from_sdk(s.receive_token),
        }
    }
}

/// Reverse direction of the existing `From<TokenId> for SdkTokenId`
/// — needed because `Swap`'s `deposit_token` / `receive_token` come
/// from the SDK as `SdkTokenId` and we project them into the FFI's
/// own enum. Inlined as a free fn so the existing `From` impl can
/// stay one-way (uniffi-bindgen doesn't need both).
fn token_id_from_sdk(t: SdkTokenId) -> TokenId {
    match t {
        SdkTokenId::Btc => TokenId::Btc,
        SdkTokenId::UsdcPolygon => TokenId::UsdcPolygon,
        SdkTokenId::UsdcArbitrum => TokenId::UsdcArbitrum,
        SdkTokenId::UsdcEthereum => TokenId::UsdcEthereum,
        SdkTokenId::UsdtPolygon => TokenId::UsdtPolygon,
        SdkTokenId::UsdtEthereum => TokenId::UsdtEthereum,
        SdkTokenId::Usdt0Arbitrum => TokenId::Usdt0Arbitrum,
        SdkTokenId::WbtcPolygon => TokenId::WbtcPolygon,
        SdkTokenId::WbtcArbitrum => TokenId::WbtcArbitrum,
        SdkTokenId::WbtcEthereum => TokenId::WbtcEthereum,
        SdkTokenId::Other(s) => TokenId::Other { wire: s },
        other => TokenId::Other {
            wire: other.as_wire_str().to_string(),
        },
    }
}

/// Create a swap.
///
/// Create-swap method block — kept separate from the constructor /
/// version / quote impls above so the new-types-then-method shape
/// reads top-down.
#[uniffi::export]
impl LendaswapClient {
    /// Create a swap. Today the SDK only supports EVM stablecoin →
    /// BTC on Arkade. The dispatcher in `Client::create_swap`
    /// validates the direction and errors with `Error::InvalidSwap`
    /// for anything else. We surface `gasless` here (the dispatcher
    /// hard-codes it to `false`) so FFI callers can opt into the
    /// gasless funding flow without dropping down to a direction-
    /// specific entry point.
    ///
    /// State note: `create_swap` writes the per-swap `key_index` into
    /// the inner client's storage. Subsequent [`Self::fund_swap_gasless`]
    /// / [`Self::claim`] calls on THIS instance recover it. A new
    /// `LendaswapClient` instance won't see it — the FFI doesn't
    /// expose a persistent storage backend yet.
    pub fn create_swap(
        &self,
        source_chain: ChainId,
        source_token: TokenId,
        target_chain: ChainId,
        target_token: TokenId,
        amount: QuoteAmount,
        receive_to: Address,
        gasless: bool,
    ) -> Result<Swap, SdkError> {
        // Direction-validation is the SDK's job — Chain here is only
        // useful as a sanity check we route correctly downstream.
        // Today only EVM-stable → Arkade-BTC is wired; the source /
        // target chain args are accepted for API symmetry and so the
        // FFI signature stays stable as more directions land.
        let _ = (source_chain, target_chain, target_token);
        runtime().block_on(async {
            let swap = self
                .inner
                .create_evm_to_arkade_swap(
                    source_token.into(),
                    amount.into(),
                    receive_to.into(),
                    gasless,
                )
                .await?;
            Ok(swap.into())
        })
    }
}
