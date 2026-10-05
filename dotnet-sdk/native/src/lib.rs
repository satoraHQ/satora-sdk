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

use bitcoin::hashes::Hash;
use bitcoin::hashes::sha256;
use lendaswap_sdk::Client;
use lendaswap_sdk::Error as SdkErrorInner;
use lendaswap_sdk::Signer;
use lendaswap_sdk::Swap as SdkSwap;
use lendaswap_sdk::SwapFunding as SdkSwapFunding;
use lendaswap_sdk::aa::GasOverrides as SdkGasOverrides;
use lendaswap_sdk::aa::GaslessOpts as SdkGaslessOpts;
use lendaswap_sdk::arkade::ArkadeConfig as SdkArkadeConfig;
use lendaswap_sdk::arkade::ArkadeSettlement as SdkArkadeSettlement;
use lendaswap_sdk::arkade::ArkadeSpend as SdkArkadeSpend;
use lendaswap_sdk::arkade::OffboardQuote as SdkOffboardQuote;
use lendaswap_sdk::arkade::VhtlcBalance as SdkVhtlcBalance;
use lendaswap_sdk::bitcoin::BitcoinSpendOptions;
use lendaswap_sdk::bitcoin::BitcoinSpendReceipt as SdkBitcoinSpendReceipt;
use lendaswap_sdk::bitcoin::EsploraBackend;
use lendaswap_sdk::types::Address as SdkAddress;
use lendaswap_sdk::types::BulkStatusResponse;
use lendaswap_sdk::types::Chain as SdkChain;
use lendaswap_sdk::types::KnownChain as SdkKnownChain;
use lendaswap_sdk::types::LightningDestination as SdkLightningDestination;
use lendaswap_sdk::types::LightningSendQuoteResponse;
use lendaswap_sdk::types::QuoteAmount as SdkQuoteAmount;
use lendaswap_sdk::types::QuoteRequest;
use lendaswap_sdk::types::RecoveryResponse;
use lendaswap_sdk::types::SwapDirection as SdkSwapDirection;
use lendaswap_sdk::types::SwapStatus as SdkSwapStatus;
use lendaswap_sdk::types::SwapStatusEntry as SdkSwapStatusEntry;
use lendaswap_sdk::types::TokenId as SdkTokenId;
use std::io::IsTerminal;
use std::sync::OnceLock;
use tracing_subscriber::EnvFilter;

uniffi::setup_scaffolding!();

/// Single shared runtime for the lifetime of the loaded shared library.
/// Multi-thread so concurrent FFI calls don't queue on one worker.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        install_stderr_logging(std::env::var("RUST_LOG").ok());
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("satora-sdk-ffi")
            .build()
            .expect("failed to build tokio runtime for satora-sdk-ffi")
    })
}

/// Writes the SDK's `tracing` events to stderr, filtered by `filter` (RUST_LOG
/// syntax). Off when `filter` is `None`, and left alone when the host process
/// already installed a global subscriber. Returns whether it installed one.
/// Public only for the test that owns its process's global subscriber.
#[doc(hidden)]
pub fn install_stderr_logging(filter: Option<String>) -> bool {
    let Some(filter) = filter else {
        return false;
    };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .try_init()
        .is_ok()
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

/// FFI-visible errors: one variant per [`lendaswap_sdk::Error`] variant, plus
/// `Internal` for failures on this side of the boundary and for SDK variants
/// not mirrored here yet. Each carries the SDK variant's own payload: `Api`
/// the HTTP status and the server's message, the others their message text.
/// `Cancelled`, which has no payload, and `Internal` carry a description.
#[derive(Debug, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum SdkError {
    #[error("{message}")]
    Cancelled { message: String },
    #[error("{message}")]
    InvalidBaseUrl { message: String },
    #[error("{message}")]
    Transport { message: String },
    #[error("{message}")]
    Decode { message: String },
    #[error("{message}")]
    Api { status: u16, message: String },
    #[error("{message}")]
    InvalidSigner { message: String },
    #[error("{message}")]
    InvalidSwap { message: String },
    #[error("{message}")]
    UnsupportedQuotePath { message: String },
    #[error("{message}")]
    Timeout { message: String },
    #[error("{message}")]
    Storage { message: String },
    #[error("{message}")]
    UnresolvedFunding { message: String },
    #[error("{message}")]
    Internal { message: String },
}

impl From<SdkErrorInner> for SdkError {
    fn from(e: SdkErrorInner) -> Self {
        match e {
            SdkErrorInner::Cancelled => Self::Cancelled {
                message: e.to_string(),
            },
            SdkErrorInner::InvalidBaseUrl(message) => Self::InvalidBaseUrl { message },
            SdkErrorInner::Transport(message) => Self::Transport { message },
            SdkErrorInner::Decode(message) => Self::Decode { message },
            SdkErrorInner::Api { status, message } => Self::Api { status, message },
            SdkErrorInner::InvalidSigner(message) => Self::InvalidSigner { message },
            SdkErrorInner::InvalidSwap(message) => Self::InvalidSwap { message },
            SdkErrorInner::UnsupportedQuotePath(message) => Self::UnsupportedQuotePath { message },
            SdkErrorInner::Timeout(message) => Self::Timeout { message },
            SdkErrorInner::Storage(message) => Self::Storage { message },
            SdkErrorInner::UnresolvedFunding(message) => Self::UnresolvedFunding { message },
            // `Error` is `#[non_exhaustive]` upstream: a new variant arrives
            // as `Internal` until it is mirrored here.
            _ => Self::Internal {
                message: e.to_string(),
            },
        }
    }
}

/// An FFI argument that does not parse, reported like the SDK's own argument
/// errors.
fn invalid_argument(message: String) -> SdkError {
    SdkErrorInner::InvalidSwap(message).into()
}

/// Whether `path` names the file at `existing`, which must exist. A path that
/// does not resolve cannot name an existing file.
fn names_existing_file(path: &std::path::Path, existing: &std::path::Path) -> bool {
    std::fs::canonicalize(existing)
        .is_ok_and(|existing| std::fs::canonicalize(path).is_ok_and(|path| path == existing))
}

/// Name component for this wallet's Arkade contract store, so wallets sharing a
/// data directory never share a store: ark-rs keys contracts by script alone,
/// and a store holding another wallet's contracts feeds that wallet's coins
/// into balance and coin selection. It is the first 16 hex characters of the
/// SHA-256 of the user-id xpub that swap storage is partitioned by, which keeps
/// the xpub itself out of file names.
#[uniffi::export]
pub fn contract_store_id(mnemonic: String) -> Result<String, SdkError> {
    let xpub = Signer::from_mnemonic(mnemonic)?.derive_user_id_xpub()?;
    let digest = sha256::Hash::hash(xpub.to_string().as_bytes()).to_string();
    Ok(digest[..16].to_owned())
}

/// Stateful FFI client — wraps one `lendaswap_sdk::Client` for its
/// lifetime so storage (swap_id → key_index) persists across calls.
/// Without that continuity, a `create_swap`/`fund_swap`/`claim` chain
/// from foreign code couldn't recover the per-swap secret material.
///
/// Constructed via [`Self::new`] (read-only, supports version + quote
/// only) or [`Self::new_signing`] (full surface — required for
/// `create_swap`, funding, and claim). Both forms use the default
/// in-memory swap storage; [`Self::new_with_arkade`] can persist it to
/// SQLite instead.
#[derive(uniffi::Object)]
pub struct SatoraClient {
    inner: Client,
    /// Esplora URL and network of the `arkade_config`
    /// [`Self::new_with_arkade`] was built with, kept because the SDK
    /// client does not expose them and [`Self::refund_bitcoin`] reads chain
    /// data there.
    esplora: Option<(String, bitcoin::Network)>,
}

#[uniffi::export]
impl SatoraClient {
    /// Read-only client. Supports [`Self::version`] and
    /// [`Self::quote`]; any signer-requiring method (`create_swap`,
    /// `fund_swap_gasless`, `claim`) errors with `InvalidSigner` when
    /// invoked through a non-signing client.
    ///
    /// `referral_code` (optional) is attached to every swap/quote
    /// originated through this client — set it once here instead of
    /// repeating it per-call.
    #[uniffi::constructor]
    pub fn new(
        base_url: String,
        referral_code: Option<String>,
    ) -> Result<std::sync::Arc<Self>, SdkError> {
        let mut inner = Client::new(&base_url)?;
        inner.set_referral_code(referral_code);
        Ok(std::sync::Arc::new(Self {
            inner,
            esplora: None,
        }))
    }

    /// Signing client. Required for any method that derives per-swap
    /// secret material (preimage, EVM key) from the mnemonic. Use
    /// [`Self::new_with_arkade`] when you need the Arkade-side methods
    /// too (claim, balance, settle, offchain_address) — those error
    /// without an arkade_config.
    ///
    /// `referral_code` (optional) is attached to every swap/quote
    /// originated through this client.
    #[uniffi::constructor]
    pub fn new_signing(
        base_url: String,
        mnemonic: String,
        referral_code: Option<String>,
    ) -> Result<std::sync::Arc<Self>, SdkError> {
        let mut builder = Client::builder().base_url(&base_url).mnemonic(&mnemonic);
        if let Some(code) = referral_code {
            builder = builder.referral_code(code);
        }
        let inner = builder.build()?;
        Ok(std::sync::Arc::new(Self {
            inner,
            esplora: None,
        }))
    }

    /// Signing client + Arkade config. Required for every Arkade method,
    /// such as `claim`, `arkade_balance`, `arkade_settle` and `create_swap`
    /// with `receive_to = None`, and for `refund_bitcoin`.
    ///
    /// `referral_code` (optional) is attached to every swap/quote
    /// originated through this client.
    ///
    /// `storage_path` (optional) keeps swap records in that SQLite file,
    /// partitioned by wallet, so a later client over the same file can
    /// still fund and claim them; its directory must exist. `None` keeps
    /// them in memory for this client's lifetime. A
    /// `arkade_config.contract_db_path` naming the same file is refused
    /// with `SdkError::Storage`: both paths are compared after resolving
    /// symlinks, `.` and `..`, and on macOS and Windows letter case.
    ///
    /// Each kind of execution through one client, such as wallet spends or
    /// swap claims, runs one at a time, and different kinds can overlap.
    /// Separate clients are not coordinated, even over the same
    /// `storage_path`: keep one client per wallet per process.
    #[uniffi::constructor]
    pub fn new_with_arkade(
        base_url: String,
        mnemonic: String,
        arkade_config: ArkadeConfig,
        referral_code: Option<String>,
        storage_path: Option<String>,
    ) -> Result<std::sync::Arc<Self>, SdkError> {
        let contract_db_path = arkade_config.contract_db_path.clone();
        let esplora = (
            arkade_config.esplora_url.clone(),
            arkade_config.network.clone().into(),
        );
        let mut builder = Client::builder()
            .base_url(&base_url)
            .mnemonic(&mnemonic)
            .arkade_config(arkade_config.into());
        if let Some(code) = referral_code {
            builder = builder.referral_code(code);
        }
        if let Some(path) = &storage_path {
            builder = builder.storage_path(path);
        }
        let inner = builder.build()?;
        // Checked once `build` has created the swap store: canonicalizing
        // an existing file is what resolves letter case on macOS and
        // Windows, which a check before the file exists would miss.
        if let (Some(swaps), Some(contracts)) = (storage_path, contract_db_path)
            && names_existing_file(contracts.as_ref(), swaps.as_ref())
        {
            return Err(SdkErrorInner::Storage(
                "arkade_config.contract_db_path must not be the storage_path database".into(),
            )
            .into());
        }
        Ok(std::sync::Arc::new(Self {
            inner,
            esplora: Some(esplora),
        }))
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
    Rootstock,
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
            ChainId::Rootstock => SdkChain::Known(SdkKnownChain::Rootstock),
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
            QuoteAmount::Source { units } => SdkQuoteAmount::Source(units.into()),
            QuoteAmount::Target { units } => SdkQuoteAmount::Target(units.into()),
        }
    }
}

// Quote method block — split from the constructor / version block so
// the chain/token/amount enum definitions can sit between them
// (uniffi::export only sees one impl block at a time, but rustc is
// fine with multiple).
#[uniffi::export]
impl SatoraClient {
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
/// `Bolt11Invoice` is the Lightning to Arkade or EVM path: the caller
/// pays the invoice from their Lightning wallet, then claims the Arkade
/// VHTLC or the EVM HTLC.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SwapFunding {
    Gasless {
        deposit_address: String,
    },
    UserSubmitted,
    /// On-chain Bitcoin HTLC address the payer funds with the deposit
    /// amount.
    BitcoinAddress {
        address: String,
    },
    Bolt11Invoice {
        invoice: String,
    },
    /// Arkade to Lightning or EVM: user funds this Arkade VHTLC address.
    ArkadeAddress {
        address: String,
    },
    /// A funding variant of the Rust SDK this binding does not name yet,
    /// carrying its debug output.
    Unknown {
        description: String,
    },
}

impl From<SdkSwapFunding> for SwapFunding {
    fn from(f: SdkSwapFunding) -> Self {
        match f {
            SdkSwapFunding::Gasless { deposit_address } => Self::Gasless { deposit_address },
            SdkSwapFunding::UserSubmitted { .. } => Self::UserSubmitted,
            SdkSwapFunding::BitcoinAddress { address } => Self::BitcoinAddress { address },
            SdkSwapFunding::Bolt11Invoice { invoice } => Self::Bolt11Invoice { invoice },
            SdkSwapFunding::ArkadeAddress { address } => Self::ArkadeAddress { address },
            // `SdkSwapFunding` is `#[non_exhaustive]` upstream: a swap
            // read back with a variant added there must not panic across
            // the FFI boundary.
            other => Self::Unknown {
                description: format!("{other:?}"),
            },
        }
    }
}

/// Route of a swap. Mirrors `lendaswap_sdk::types::SwapDirection`.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SwapDirection {
    BtcToArkade,
    BitcoinToEvm,
    ArkadeToEvm,
    LightningToEvm,
    EvmToBitcoin,
    EvmToLightning,
    EvmToArkade,
    LightningToArkade,
    ArkadeToLightning,
    /// A direction of the Rust SDK this binding does not name yet,
    /// carrying its debug output.
    Unknown {
        description: String,
    },
}

impl From<SdkSwapDirection> for SwapDirection {
    fn from(d: SdkSwapDirection) -> Self {
        match d {
            SdkSwapDirection::BtcToArkade => Self::BtcToArkade,
            SdkSwapDirection::BitcoinToEvm => Self::BitcoinToEvm,
            SdkSwapDirection::ArkadeToEvm => Self::ArkadeToEvm,
            SdkSwapDirection::LightningToEvm => Self::LightningToEvm,
            SdkSwapDirection::EvmToBitcoin => Self::EvmToBitcoin,
            SdkSwapDirection::EvmToLightning => Self::EvmToLightning,
            SdkSwapDirection::EvmToArkade => Self::EvmToArkade,
            SdkSwapDirection::LightningToArkade => Self::LightningToArkade,
            SdkSwapDirection::ArkadeToLightning => Self::ArkadeToLightning,
            // `SdkSwapDirection` is `#[non_exhaustive]` upstream.
            other => Self::Unknown {
                description: format!("{other:?}"),
            },
        }
    }
}

/// Compact, user-facing view of a created swap. Mirrors
/// `lendaswap_sdk::Swap` — amounts stay as strings to preserve
/// precision for large EVM token amounts.
///
/// Locktimes and expiries are unix seconds, `None` when the direction
/// has no such leg. A locktime on the leg the client funds is when the
/// client's refund unlocks. On the leg the server funds, the server can
/// refund from then, so it is the client's deadline to claim. Bitcoin and
/// Arkade locks mature on median time past, which trails wall clock by
/// about an hour, so a refund at the locktime can still be refused;
/// [`SatoraClient::claim`] refuses within 300 seconds of the VHTLC
/// locktime.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Swap {
    pub id: String,
    pub status: SwapStatus,
    pub direction: SwapDirection,
    pub funding: SwapFunding,
    pub deposit_amount: String,
    pub deposit_token: TokenId,
    pub receive_address: String,
    pub receive_amount: String,
    pub receive_token: TokenId,
    /// Server creation time as sent on the wire (RFC 3339).
    pub created_at: String,
    /// Arkade VHTLC refund locktime. Bitcoin, EVM or Lightning to Arkade:
    /// the claim deadline. Arkade to EVM or Lightning: when the client's
    /// refund unlocks.
    pub vhtlc_refund_locktime: Option<i64>,
    /// On-chain Bitcoin HTLC refund locktime. Bitcoin to Arkade or EVM:
    /// when the client's refund unlocks. EVM to Bitcoin: the claim
    /// deadline.
    pub btc_refund_locktime: Option<i64>,
    /// EVM HTLC refund locktime. EVM to Arkade, Bitcoin or Lightning: when
    /// the client's refund unlocks. Bitcoin, Arkade or Lightning to EVM:
    /// the claim deadline.
    pub evm_refund_locktime: Option<i64>,
    /// Expiry of the invoice the payer pays (Lightning to Arkade or EVM)
    /// or the server pays (Arkade or EVM to Lightning). `None` also when
    /// the server reports no expiry, which it sends as 0. A payer must pay
    /// before both this expiry and the claim cutoff on the server-funded
    /// leg (`vhtlc_refund_locktime` for Lightning to Arkade,
    /// `evm_refund_locktime` for Lightning to EVM); the server does not
    /// guarantee which comes first.
    pub invoice_expires_at: Option<i64>,
}

impl From<SdkSwap> for Swap {
    fn from(s: SdkSwap) -> Self {
        Self {
            id: s.id,
            status: s.status.into(),
            direction: s.direction.into(),
            funding: s.funding.into(),
            deposit_amount: s.deposit_amount,
            deposit_token: token_id_from_sdk(s.deposit_token),
            receive_address: s.receive_address,
            receive_amount: s.receive_amount,
            receive_token: token_id_from_sdk(s.receive_token),
            created_at: s.created_at,
            vhtlc_refund_locktime: s.vhtlc_refund_locktime,
            btc_refund_locktime: s.btc_refund_locktime,
            evm_refund_locktime: s.evm_refund_locktime,
            invoice_expires_at: s.invoice_expires_at,
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

impl SatoraClient {
    async fn resolve_receive_to(
        &self,
        receive_to: Option<Address>,
    ) -> Result<SdkAddress, SdkError> {
        match receive_to {
            Some(address) => Ok(address.into()),
            None => Ok(SdkAddress::Arkade(
                self.inner.arkade_offchain_address().await?,
            )),
        }
    }
}

/// Create a swap.
///
/// Create-swap method block — kept separate from the constructor /
/// version / quote impls above so the new-types-then-method shape
/// reads top-down.
#[uniffi::export]
impl SatoraClient {
    /// Create a swap. Thin forward to the Rust SDK's dispatcher
    /// ([`lendaswap_sdk::Client::create_swap`]). Direction is resolved
    /// from `source_chain` (Lightning, Bitcoin or EVM) plus the target.
    /// Bitcoin needs `QuoteAmount::Target`. See the underlying SDK doc
    /// for the supported directions today.
    ///
    /// `gasless` is honoured only when the routed direction is EVM →
    /// Arkade — the Lightning rail doesn't model gasless funding (the
    /// user just pays the Bolt11 invoice from their own wallet). The
    /// FFI signature stays uniform so the C# surface doesn't need an
    /// overload per direction; the inner dispatcher does the routing.
    ///
    /// A Lightning swap is checked as the dedicated Lightning methods
    /// describe before anything is stored.
    ///
    /// State note: `create_swap` writes the per-swap `key_index` into
    /// the inner client's storage. Subsequent [`Self::fund_swap_gasless`]
    /// / [`Self::claim`] calls on THIS instance recover it. A new
    /// `SatoraClient` instance sees it only when both were built with
    /// the same `storage_path`.
    #[allow(
        clippy::too_many_arguments,
        reason = "FFI surface — mirrors the inner SDK signature so uniffi-bindgen-cs maps each arg 1:1 to a C# parameter"
    )]
    pub fn create_swap(
        &self,
        source_chain: ChainId,
        source_token: TokenId,
        target_chain: ChainId,
        target_token: TokenId,
        amount: QuoteAmount,
        // `None` => derive the destination from the SDK's internal
        // Arkade wallet (requires the client to have been built via
        // `new_with_arkade`). `Some(addr)` => use that address directly.
        receive_to: Option<Address>,
        gasless: bool,
        // Optional per-swap fee surcharge in basis points
        // (0..=max_extra_fee_bps configured on the matching dev key),
        // forwarded for every routed direction. `None` => the key's
        // configured default applies server-side.
        extra_fees_bps: Option<u16>,
    ) -> Result<Swap, SdkError> {
        // target_chain / target_token are accepted for FFI symmetry but
        // not used for routing today (only Arkade-BTC is a valid target).
        let _ = (target_chain, target_token);
        // The dispatcher hard-codes `gasless = false`. To preserve the
        // FFI's `gasless` knob we drop to the direction-specific EVM
        // method for the EVM rail; Lightning and Bitcoin route through
        // the dispatcher (which doesn't take `gasless`).
        let sdk_source_chain = match source_chain {
            ChainId::Polygon => SdkKnownChain::Polygon,
            ChainId::Ethereum => SdkKnownChain::Ethereum,
            ChainId::Arbitrum => SdkKnownChain::Arbitrum,
            ChainId::Rootstock => SdkKnownChain::Rootstock,
            ChainId::Arkade => SdkKnownChain::Arkade,
            ChainId::Lightning => SdkKnownChain::Lightning,
            ChainId::Bitcoin => SdkKnownChain::Bitcoin,
            ChainId::Other { ref wire } => {
                return Err(invalid_argument(format!(
                    "unsupported source_chain Other({wire})"
                )));
            }
        };
        runtime().block_on(async {
            let swap = if matches!(source_chain, ChainId::Lightning | ChainId::Bitcoin) {
                self.inner
                    .create_swap(
                        sdk_source_chain,
                        source_token.into(),
                        SdkTokenId::Btc,
                        amount.into(),
                        receive_to.map(Into::into),
                        extra_fees_bps,
                    )
                    .await?
            } else {
                let resolved = self.resolve_receive_to(receive_to).await?;
                self.inner
                    .create_evm_to_arkade_swap(
                        source_token.into(),
                        amount.into(),
                        resolved,
                        gasless,
                        extra_fees_bps,
                    )
                    .await?
            };
            Ok(swap.into())
        })
    }
}

// ─── Lightning → Arkade ───────────────────────────────────────────────

#[uniffi::export]
impl SatoraClient {
    /// Create a Lightning → Arkade swap. Dedicated counterpart to
    /// [`Self::create_arkade_to_lightning_swap`]: the server returns a
    /// BOLT11 invoice (in `swap.funding` as `SwapFunding::Bolt11Invoice`)
    /// that the user pays from their own Lightning wallet; the server
    /// then funds an Arkade VHTLC the user claims via [`Self::claim`].
    ///
    /// `sats_receive` is what the user wants to *receive* on Arkade —
    /// the server quotes the corresponding BOLT11 amount internally and
    /// embeds it in the invoice.
    ///
    /// `receive_to` is the Arkade address that receives the BTC. Pass
    /// `None` to route to the SDK's own internal Arkade wallet (requires
    /// the client to have been built via `new_with_arkade`), mirroring
    /// [`Self::create_swap`]'s address-less behaviour.
    ///
    /// This is the same direction [`Self::create_swap`] routes when
    /// `source_chain == Lightning`; the dedicated method exists so the
    /// C# surface has a discoverable, single-purpose entry point that
    /// matches the Rust SDK's `create_lightning_to_arkade_swap`.
    ///
    /// A server invoice that does not pay the hash of this client's
    /// preimage fails with `SdkError::InvalidSwap`, and nothing is stored:
    /// paying it would let the server keep the payment without funding
    /// the VHTLC.
    pub fn create_lightning_to_arkade_swap(
        &self,
        sats_receive: u64,
        receive_to: Option<Address>,
    ) -> Result<Swap, SdkError> {
        runtime().block_on(async {
            let resolved = self.resolve_receive_to(receive_to).await?;
            let swap = self
                .inner
                .create_lightning_to_arkade_swap(sats_receive, resolved)
                .await?;
            Ok(swap.into())
        })
    }

    /// [`Self::create_lightning_to_arkade_swap`] with an exact source or
    /// target amount: `QuoteAmount::Source` fixes the invoice amount,
    /// `QuoteAmount::Target` what arrives on Arkade, both in sats.
    /// `receive_to` `None` routes to the SDK's own internal Arkade wallet.
    /// `extra_fees_bps` is the optional per-swap fee surcharge
    /// [`Self::create_swap`] takes. `invoice_description` is at most 639
    /// UTF-8 bytes; `None` uses the server default.
    pub fn create_lightning_to_arkade_swap_with_options(
        &self,
        amount: QuoteAmount,
        receive_to: Option<Address>,
        extra_fees_bps: Option<u16>,
        invoice_description: Option<String>,
    ) -> Result<Swap, SdkError> {
        runtime().block_on(async {
            let resolved = self.resolve_receive_to(receive_to).await?;
            let swap = self
                .inner
                .create_lightning_to_arkade_swap_with_options(
                    amount.into(),
                    resolved,
                    extra_fees_bps,
                    invoice_description,
                )
                .await?;
            Ok(swap.into())
        })
    }
}

// ─── Bitcoin → Arkade ─────────────────────────────────────────────────

/// Result of an on-chain Bitcoin HTLC refund.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct BitcoinRefundReceipt {
    /// Refund transaction ID, hex as block explorers show it (no `0x`).
    pub txid: String,
    pub fee_sats: u64,
}

impl From<SdkBitcoinSpendReceipt> for BitcoinRefundReceipt {
    fn from(r: SdkBitcoinSpendReceipt) -> Self {
        Self {
            txid: r.transaction.compute_txid().to_string(),
            fee_sats: r.fee.to_sat(),
        }
    }
}

#[uniffi::export]
impl SatoraClient {
    /// Create an on-chain Bitcoin → Arkade swap. The payer sends the
    /// deposit amount to the HTLC address in `swap.funding`
    /// (`SwapFunding::BitcoinAddress`); once the server funds the Arkade
    /// VHTLC, the user claims it via [`Self::claim`]. If the swap does
    /// not complete, [`Self::refund_bitcoin`] returns the deposit once
    /// the HTLC's refund locktime has matured.
    ///
    /// `sats_receive` is the exact amount to receive on Arkade.
    /// `receive_to` `None` routes to the SDK's own internal Arkade wallet
    /// (requires the client to have been built via `new_with_arkade`),
    /// like [`Self::create_lightning_to_arkade_swap`]. `extra_fees_bps`
    /// is the optional per-swap fee surcharge [`Self::create_swap`] takes.
    ///
    /// A server HTLC address that differs from the one the client's keys
    /// rebuild is refused with `SdkError::InvalidSwap`, here and through
    /// [`Self::create_swap`] alike.
    pub fn create_bitcoin_to_arkade_swap(
        &self,
        sats_receive: u64,
        receive_to: Option<Address>,
        extra_fees_bps: Option<u16>,
    ) -> Result<Swap, SdkError> {
        runtime().block_on(async {
            let resolved = self.resolve_receive_to(receive_to).await?;
            let swap = self
                .inner
                .create_bitcoin_to_arkade_swap(sats_receive, resolved, extra_fees_bps)
                .await?;
            Ok(swap.into())
        })
    }

    /// Refund the on-chain HTLC of a Bitcoin → Arkade swap to
    /// `destination`, a Bitcoin address on the client's network, once the
    /// HTLC's refund locktime has matured. Wraps
    /// [`lendaswap_sdk::Client::refund_bitcoin`] over the Esplora endpoint
    /// of `arkade_config`, so only a client built via
    /// [`Self::new_with_arkade`] can refund; others get
    /// `SdkError::InvalidSwap`.
    ///
    /// `fee_rate_sat_vb` `None` takes Mempool's recommended hour fee from
    /// the endpoint, or plain esplora's estimate for confirmation within six
    /// blocks, never below 1 sat/vB. HTLC outputs whose fee would push
    /// the refund over `maximum_fee_sats` are left for a later call; if
    /// not even one fits, the call is refused with `SdkError::InvalidSwap`.
    /// Unconfirmed HTLC outputs are refunded too, and the call gives up
    /// with `SdkError::Timeout` after 60 seconds.
    ///
    /// Once a refund is pending, a call returns it unchanged unless
    /// `fee_rate_sat_vb` is at least 1 sat/vB above its rate and the
    /// replacement's fee stays within `maximum_fee_sats`; compare the
    /// returned txid or fee to see whether it was bumped. Once a refund to
    /// one address exists, a different `destination` is refused with
    /// `SdkError::InvalidSwap`.
    ///
    /// A `Transport` or `Timeout` error, or a `Storage` error after a fee
    /// bump, leaves the outcome unknown: the refund may have been
    /// broadcast. Call again with the same `destination`, which returns
    /// the pending or confirmed refund instead of building another.
    pub fn refund_bitcoin(
        &self,
        swap_id: String,
        destination: String,
        fee_rate_sat_vb: Option<u64>,
        maximum_fee_sats: u64,
    ) -> Result<BitcoinRefundReceipt, SdkError> {
        let (esplora_url, network) = self.esplora.as_ref().ok_or_else(|| {
            SdkError::from(SdkErrorInner::InvalidSwap(
                "refund_bitcoin needs a client built with new_with_arkade".into(),
            ))
        })?;
        let fee_rate = fee_rate_sat_vb
            .map(|rate| {
                bitcoin::FeeRate::from_sat_per_vb(rate)
                    .ok_or_else(|| invalid_argument(format!("fee_rate_sat_vb {rate} overflows")))
            })
            .transpose()?;
        let options = BitcoinSpendOptions {
            destination,
            fee_rate,
            maximum_fee: bitcoin::Amount::from_sat(maximum_fee_sats),
            minimum_confirmations: 0,
            minimum_remaining_blocks: 0,
            confirmation_target: 6,
            timeout: std::time::Duration::from_secs(60),
        };
        runtime().block_on(async {
            let backend = EsploraBackend::new(esplora_url, *network)?;
            let receipt = self
                .inner
                .refund_bitcoin(&swap_id, &backend, options)
                .await?;
            Ok(receipt.into())
        })
    }
}

// ─── Arkade → Lightning ───────────────────────────────────────────────

/// Lightning destination for an Arkade → Lightning swap. The server
/// resolves `Address` / `Lnurl` via LNURL-pay using `sats`, the payout,
/// and the send-max variants using `amount`, the sats the swap takes from
/// Arkade, paying out what remains after fees; `Invoice` carries its
/// amount in the BOLT11 payload.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LightningDestination {
    /// BOLT11 invoice. Server pays this exact invoice.
    Invoice { invoice: String },
    /// Lightning address (`user@host`). Server resolves via LUD-16.
    Address { address: String, sats: u64 },
    /// Raw LNURL (`lnurl1…`). Server bech32-decodes + resolves via
    /// LNURL-pay.
    Lnurl { lnurl: String, sats: u64 },
    /// Lightning address with a fixed source amount in sats.
    AddressSendMax { address: String, amount: u64 },
    /// Raw LNURL with a fixed source amount in sats.
    LnurlSendMax { lnurl: String, amount: u64 },
}

impl From<LightningDestination> for SdkLightningDestination {
    fn from(d: LightningDestination) -> Self {
        match d {
            LightningDestination::Invoice { invoice } => SdkLightningDestination::Invoice(invoice),
            LightningDestination::Address { address, sats } => {
                SdkLightningDestination::Address { address, sats }
            }
            LightningDestination::Lnurl { lnurl, sats } => {
                SdkLightningDestination::Lnurl { lnurl, sats }
            }
            LightningDestination::AddressSendMax { address, amount } => {
                SdkLightningDestination::AddressSendMax {
                    address,
                    amount: amount.into(),
                }
            }
            LightningDestination::LnurlSendMax { lnurl, amount } => {
                SdkLightningDestination::LnurlSendMax {
                    lnurl,
                    amount: amount.into(),
                }
            }
        }
    }
}

/// Exact quote for paying a Lightning destination from Arkade. Mirrors
/// `lendaswap_sdk::types::LightningSendQuoteResponse`.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct LightningSendQuote {
    /// Source tokens per BTC locked; "1" for Arkade, where the source is
    /// already sats.
    pub exchange_rate: String,
    /// Maximum payout for this route, in sats.
    pub max_amount_sats: u64,
    /// Minimum payout for this route, in sats.
    pub min_amount_sats: u64,
    /// The Lightning provider's send fee for this destination, in sats.
    pub network_fee_sats: u64,
    /// Protocol fee rate (0.003 = 0.30%).
    pub protocol_fee_rate: f64,
    pub protocol_fee_sats: u64,
    /// Total taken from Arkade (payout plus fees), in sats, as a decimal
    /// string.
    pub source_amount: String,
    /// Deprecated by the server in favour of `source_amount`.
    pub source_amount_sats: u64,
    /// Paid out on the Lightning invoice, in sats.
    pub target_amount_sats: u64,
}

impl From<LightningSendQuoteResponse> for LightningSendQuote {
    fn from(q: LightningSendQuoteResponse) -> Self {
        Self {
            exchange_rate: q.exchange_rate,
            max_amount_sats: q.max_amount_sats,
            min_amount_sats: q.min_amount_sats,
            network_fee_sats: q.network_fee_sats,
            protocol_fee_rate: q.protocol_fee_rate,
            protocol_fee_sats: q.protocol_fee_sats,
            source_amount: q.source_amount,
            source_amount_sats: q.source_amount_sats,
            target_amount_sats: q.target_amount_sats,
        }
    }
}

#[uniffi::export]
impl SatoraClient {
    /// Quote paying `destination` from Arkade BTC with the Lightning
    /// provider's fee for that destination, unlike [`Self::quote`], whose
    /// Lightning fee is an estimate. Informational: creating the swap
    /// quotes the fee again, and it can move in between. `extra_fees_bps`
    /// is the optional per-swap fee surcharge [`Self::create_swap`] takes;
    /// [`Self::create_arkade_to_lightning_swap`] has no surcharge parameter
    /// yet, so pass `None` to quote like for like.
    pub fn get_lightning_send_quote(
        &self,
        destination: LightningDestination,
        extra_fees_bps: Option<u16>,
    ) -> Result<LightningSendQuote, SdkError> {
        runtime().block_on(async {
            let quote = self
                .inner
                .get_lightning_send_quote(
                    SdkKnownChain::Arkade,
                    SdkTokenId::Btc,
                    destination.into(),
                    extra_fees_bps,
                )
                .await?;
            Ok(quote.into())
        })
    }

    /// Create an Arkade → Lightning swap. The server returns the
    /// Arkade VHTLC address the user must fund (in `swap.funding` as
    /// `SwapFunding::ArkadeAddress`); the server (via Boltz) then pays
    /// the Lightning destination and claims the VHTLC. No client-side
    /// claim — see the underlying SDK doc.
    ///
    /// A supplied invoice that does not parse fails with
    /// `SdkError::InvalidSwap` before any request is sent. A hash lock that
    /// is not the payment hash of the returned invoice, or a returned
    /// invoice other than the supplied one, fails the same way, and nothing
    /// is stored. For a Lightning address or LNURL the server picks the
    /// invoice, so only their agreement is checked.
    pub fn create_arkade_to_lightning_swap(
        &self,
        destination: LightningDestination,
    ) -> Result<Swap, SdkError> {
        runtime().block_on(async {
            let swap = self
                .inner
                .create_arkade_to_lightning_swap(destination.into())
                .await?;
            Ok(swap.into())
        })
    }
}

// ─── Gasless funding ───────────────────────────────────────────────────

/// Caller-supplied bits for the gasless funding flow. Bundler URL,
/// paymaster URL, EntryPoint, delegation target, and Permit2 address
/// all come from the backend's `/aa/config` endpoint — they don't
/// appear here. What the SDK can't infer:
/// - which EVM node to talk to (depends on the consumer's provider stack);
/// - paymaster context as a JSON string (depends on the paymaster's policy);
/// - whether to bypass `eth_estimateUserOperationGas` and with what limits.
#[derive(uniffi::Record, Clone, Debug)]
pub struct GaslessOpts {
    /// EVM node RPC URL (e.g. Alchemy / Infura). `None` means
    /// "pick the chain's default" — the SDK resolves it at fund
    /// time from the swap's deposit chain. Set explicitly only
    /// when overriding the public-RPC default.
    pub node_rpc_url: Option<String>,
    /// Paymaster context as a JSON string (e.g.
    /// `'{"policyId":"<uuid>"}'` for Alchemy Gas Manager). Ignored when
    /// the backend returns no paymaster. `None` => JSON null.
    pub paymaster_context_json: Option<String>,
    /// Bypass `eth_estimateUserOperationGas` with explicit limits.
    /// Useful when the bundler's simulator mis-estimates (notably
    /// alto in dev). `None` => let the bundler estimate.
    pub gas_overrides: Option<GasOverrides>,
}

/// Explicit gas limits for the userOp. When set on [`GaslessOpts`], the
/// SDK skips the bundler's gas estimation entirely. Typical values
/// for a USDC→tBTC gasless swap on Arbitrum: call ~500_000,
/// verification ~150_000, pre_verification ~100_000.
#[derive(uniffi::Record, Clone, Copy, Debug)]
pub struct GasOverrides {
    pub call_gas_limit: u64,
    pub verification_gas_limit: u64,
    pub pre_verification_gas: u64,
}

impl From<GasOverrides> for SdkGasOverrides {
    fn from(g: GasOverrides) -> Self {
        Self {
            call_gas_limit: g.call_gas_limit,
            verification_gas_limit: g.verification_gas_limit,
            pre_verification_gas: g.pre_verification_gas,
        }
    }
}

/// Single-shot snapshot of the depositor EOA's funding state.
/// `source_token_balance` and `source_token_required` are decimal
/// strings (U256 doesn't fit in a uniffi-native int); native balance
/// fits in u64 in practice.
#[derive(uniffi::Record, Clone, Debug)]
pub struct DepositStatus {
    /// Current source-token balance at the depositor EOA, smallest unit.
    pub source_token_balance: String,
    /// Amount the depositor EOA must hold before the funding userOp
    /// can be submitted, smallest unit.
    pub source_token_required: String,
    /// Native (ETH / MATIC / …) balance at the depositor EOA, wei.
    pub native_balance_wei: u64,
    /// `source_token_balance >= source_token_required`.
    pub has_sufficient_source_token: bool,
}

/// Submitted-userOp receipt. `transaction_hash` is `None` when the
/// SDK's bounded receipt poll ran out — callers can re-poll via
/// the bundler directly if needed.
#[derive(uniffi::Record, Clone, Debug)]
pub struct FundSwapReceipt {
    pub user_op_hash: String,
    pub transaction_hash: Option<String>,
}

/// Translate the FFI's URL-string opts into the SDK's typed shape.
/// Returns `SdkError::InvalidSwap` for malformed URL / paymaster JSON.
/// A `None` node_rpc_url passes through — the SDK fund function
/// resolves the per-chain default at fund time.
fn gasless_opts_into_sdk(o: GaslessOpts) -> Result<SdkGaslessOpts, SdkError> {
    use url::Url;
    let node_rpc_url = o
        .node_rpc_url
        .as_deref()
        .map(Url::parse)
        .transpose()
        .map_err(|e| invalid_argument(format!("GaslessOpts.node_rpc_url parse: {e}")))?;
    let paymaster_context = o
        .paymaster_context_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e: serde_json::Error| {
            invalid_argument(format!("GaslessOpts.paymaster_context_json parse: {e}"))
        })?;
    let mut sdk_opts = SdkGaslessOpts {
        node_rpc_url,
        ..Default::default()
    };
    if let Some(ctx) = paymaster_context {
        sdk_opts = sdk_opts.with_paymaster_context(ctx);
    }
    if let Some(g) = o.gas_overrides {
        sdk_opts = sdk_opts.with_gas_overrides(g.into());
    }
    Ok(sdk_opts)
}

#[uniffi::export]
impl SatoraClient {
    /// Poll until the gasless deposit address holds enough source
    /// token AND enough native gas. Resolves the deposit address +
    /// required token amount from the swap response itself; the
    /// caller only supplies the gas headroom they want in wei.
    ///
    /// For an unsponsored userOp (no paymaster), ~0.001 ETH (= 1e15
    /// wei) is enough headroom on Arbitrum. With a paymaster, pass 0.
    ///
    /// Returns `SdkError::Timeout` if `timeout_seconds` elapses before
    /// both thresholds are met.
    pub fn wait_for_deposit_funding(
        &self,
        swap_id: String,
        node_rpc_url: String,
        min_eth_wei: u64,
        timeout_seconds: u64,
    ) -> Result<(), SdkError> {
        use url::Url;
        let node_rpc_url = Url::parse(&node_rpc_url)
            .map_err(|e| invalid_argument(format!("node_rpc_url parse: {e}")))?;
        runtime().block_on(async {
            self.inner
                .wait_for_deposit_funding(
                    &swap_id,
                    &node_rpc_url,
                    min_eth_wei,
                    std::time::Duration::from_secs(timeout_seconds),
                )
                .await?;
            Ok(())
        })
    }

    /// Single-shot read of the depositor EOA's funding state — no
    /// polling, no waiting, no throw on "not funded yet". Use this
    /// for "is `fund_swap_gasless` ready?" branching.
    ///
    /// `node_rpc_url` defaults to the chain-specific public RPC when
    /// `None` (same defaults the fund call uses).
    pub fn check_deposit_funding(
        &self,
        swap_id: String,
        node_rpc_url: Option<String>,
    ) -> Result<DepositStatus, SdkError> {
        use url::Url;
        let node_rpc_url = node_rpc_url
            .as_deref()
            .map(Url::parse)
            .transpose()
            .map_err(|e| invalid_argument(format!("node_rpc_url parse: {e}")))?;
        runtime().block_on(async {
            let status = self
                .inner
                .check_deposit_funding(&swap_id, node_rpc_url)
                .await?;
            // Token amounts go over the wire as decimal strings —
            // U256 doesn't fit in any uniffi-native integer type.
            // Native balance fits in u64 in practice (u64::MAX wei ≈
            // 18.45 ETH; deposit EOAs hold a few millis at most).
            Ok(DepositStatus {
                source_token_balance: status.source_token_balance.to_string(),
                source_token_required: status.source_token_required.to_string(),
                native_balance_wei: u64::try_from(status.native_balance_wei).map_err(|_| {
                    SdkError::Internal {
                        message: format!(
                            "native_balance_wei {} exceeds u64 — deposit EOA holds > 18.45 ETH which is unexpected",
                            status.native_balance_wei,
                        ),
                    }
                })?,
                has_sufficient_source_token: status.has_sufficient_source_token,
            })
        })
    }

    /// Submit the gasless ERC-4337 + EIP-7702 funding userOp for a
    /// previously-created swap. The depositor EOA must already hold
    /// the source token (real users transfer it in; e2e harnesses
    /// pre-seed via Anvil helpers).
    ///
    /// Requires the client to have been built via [`Self::new_signing`]
    /// — the SDK needs the mnemonic to re-derive the per-swap secret
    /// material and sign the userOp.
    ///
    /// A funding that may already have been submitted without a recorded
    /// hash, or a relay funding signature that may still be used, fails with
    /// `SdkError::UnresolvedFunding`; one whose hash is recorded fails with
    /// `SdkError::InvalidSwap`.
    pub fn fund_swap_gasless(
        &self,
        swap_id: String,
        opts: GaslessOpts,
    ) -> Result<FundSwapReceipt, SdkError> {
        let sdk_opts = gasless_opts_into_sdk(opts)?;
        runtime().block_on(async {
            let receipt = self.inner.fund_swap_gasless(&swap_id, sdk_opts).await?;
            Ok(FundSwapReceipt {
                user_op_hash: format!("{:#x}", receipt.user_op_hash),
                transaction_hash: receipt.transaction_hash.map(|h| format!("{h:#x}")),
            })
        })
    }
}

// ─── Status polling ────────────────────────────────────────────────────

/// One swap's status in a [`BulkStatus`]. Mirrors
/// `lendaswap_sdk::types::SwapStatusEntry`.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SwapStatusEntry {
    pub id: String,
    pub status: SwapStatus,
}

impl From<SdkSwapStatusEntry> for SwapStatusEntry {
    fn from(e: SdkSwapStatusEntry) -> Self {
        Self {
            id: e.id,
            status: e.status.into(),
        }
    }
}

/// Statuses of the swaps [`SatoraClient::get_bulk_status`] was asked
/// about. Mirrors `lendaswap_sdk::types::BulkStatusResponse`.
#[derive(uniffi::Record, Clone, Debug, Default, PartialEq, Eq)]
pub struct BulkStatus {
    pub statuses: Vec<SwapStatusEntry>,
    /// Requested IDs the server does not know.
    pub not_found: Vec<String>,
}

/// Most IDs one `POST /swap/bulk-status` request accepts.
const BULK_STATUS_CHUNK: usize = 100;

/// Queries `ids` in requests of at most [`BULK_STATUS_CHUNK`], in order,
/// and concatenates the answers.
async fn bulk_status_in_chunks<'a, F, Fut>(
    ids: &'a [String],
    mut fetch: F,
) -> Result<BulkStatus, SdkError>
where
    F: FnMut(&'a [String]) -> Fut,
    Fut: std::future::Future<Output = lendaswap_sdk::Result<BulkStatusResponse>>,
{
    let mut merged = BulkStatus::default();
    for chunk in ids.chunks(BULK_STATUS_CHUNK) {
        let page = fetch(chunk).await?;
        merged
            .statuses
            .extend(page.statuses.into_iter().map(Into::into));
        merged.not_found.extend(page.not_found);
    }
    Ok(merged)
}

#[uniffi::export]
impl SatoraClient {
    /// Current status of each swap in `ids`, any number of them, in
    /// requests of at most 100 IDs. Read-only, so safe to call again
    /// after any error.
    pub fn get_bulk_status(&self, ids: Vec<String>) -> Result<BulkStatus, SdkError> {
        runtime().block_on(bulk_status_in_chunks(&ids, |chunk| {
            self.inner.get_bulk_status(chunk)
        }))
    }

    /// `GET /swap/{id}` — fetch a swap's current state. Returns the
    /// same `Swap` shape `create_swap` does, so callers can re-read
    /// after the backend transitions states (e.g. ServerFunded).
    ///
    /// A server copy that names another swap than `swap_id`, or that
    /// changes the direction of a swap this client stored, fails with
    /// `SdkError::InvalidSwap`, and so does one whose invoice no longer
    /// pays the hash lock of a Lightning swap this client stored: never pay
    /// or fund the swap then. A swap stored only as its key index, as in an
    /// older database, gets the invoice checks recovery applies: a Lightning
    /// to Arkade invoice must pay the hash of this client's preimage, and an
    /// Arkade to Lightning hash lock must be its invoice's payment hash.
    pub fn get_swap(&self, swap_id: String) -> Result<Swap, SdkError> {
        runtime().block_on(async {
            let swap = self.inner.get_swap(&swap_id).await?;
            Ok(swap.into())
        })
    }

    /// Poll `GET /swap/{id}` until the status matches one of `targets`
    /// or `timeout_seconds` elapses (returns `SdkError::Timeout` in that
    /// case). 3s poll interval, fixed inside the SDK.
    ///
    /// `timeout_seconds: u64` because uniffi's `Duration` support is
    /// patchy across foreign bindgens; seconds is precise enough for
    /// swap-status polling (sub-second timeouts make no sense here).
    ///
    /// Each poll is checked as [`Self::get_swap`] describes, and a server
    /// copy it refuses ends the wait with `SdkError::InvalidSwap`.
    pub fn wait_for_swap_status(
        &self,
        swap_id: String,
        targets: Vec<SwapStatus>,
        timeout_seconds: u64,
    ) -> Result<SwapStatus, SdkError> {
        let sdk_targets: Vec<SdkSwapStatus> =
            targets.into_iter().map(swap_status_into_sdk).collect();
        let timeout = std::time::Duration::from_secs(timeout_seconds);
        runtime().block_on(async {
            let reached = self
                .inner
                .wait_for_swap_status(&swap_id, &sdk_targets, timeout)
                .await?;
            Ok(reached.into())
        })
    }
}

// ─── Recovery ──────────────────────────────────────────────────────────

/// Swaps [`SatoraClient::recover_all_swaps`] found. Mirrors
/// `lendaswap_sdk::types::RecoveryResponse`, minus the swaps themselves.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct RecoveryResult {
    /// IDs of the recovered swaps, now in this client's storage.
    pub swap_ids: Vec<String>,
    /// Wallet key index the next new swap uses.
    pub next_index: u32,
    /// Last wallet key index the scan covered. A later scan for newer swaps
    /// starts at `next_index` (or the original start index), not after
    /// this one.
    pub scanned_until: u32,
}

impl From<RecoveryResponse> for RecoveryResult {
    fn from(r: RecoveryResponse) -> Self {
        Self {
            swap_ids: r.swaps.iter().map(|s| s.swap.id().to_owned()).collect(),
            next_index: r.next_index,
            scanned_until: r.scanned_until,
        }
    }
}

#[uniffi::export]
impl SatoraClient {
    /// Find this wallet's swaps on the server, from wallet key index
    /// `start_index` (`None` for 0) until the server's gap limit, and
    /// store each with its key index so this client can claim and refund
    /// it. Only the wallet's public recovery key is sent. Needs a signing
    /// client.
    ///
    /// Pages already processed stay stored if a later one fails, and a
    /// swap stored again keeps its progress, so call again after a
    /// `Transport` or `Timeout` error. `SdkError::InvalidSwap` refuses a
    /// page holding a swap that fails this client's checks, such as a
    /// Lightning swap whose invoice does not pay its hash lock: nothing
    /// from that page is stored, and calling again refuses it again.
    pub fn recover_all_swaps(&self, start_index: Option<u32>) -> Result<RecoveryResult, SdkError> {
        runtime().block_on(async {
            let recovery = self.inner.recover_all_swaps(start_index).await?;
            Ok(recovery.into())
        })
    }
}

// ─── Arkade claim ──────────────────────────────────────────────────────

/// Bitcoin network the Arkade VHTLC was created on. Mirrors a
/// subset of `bitcoin::Network` — only the variants the Arkade
/// stack realistically exercises today.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BitcoinNetwork {
    Mainnet,
    Testnet,
    Signet,
    Regtest,
}

impl From<BitcoinNetwork> for bitcoin::Network {
    fn from(n: BitcoinNetwork) -> Self {
        match n {
            BitcoinNetwork::Mainnet => bitcoin::Network::Bitcoin,
            BitcoinNetwork::Testnet => bitcoin::Network::Testnet,
            BitcoinNetwork::Signet => bitcoin::Network::Signet,
            BitcoinNetwork::Regtest => bitcoin::Network::Regtest,
        }
    }
}

/// Arkade-side configuration for [`SatoraClient::claim`]. The
/// mnemonic here is the user's Arkade identity (BIP-85 derivation
/// under the SDK's hard-coded path) — distinct from the lendaswap
/// signing mnemonic the client was constructed with.
#[derive(uniffi::Record, Clone, Debug)]
pub struct ArkadeConfig {
    /// gRPC endpoint of the Arkade server (`arkd`).
    pub arkade_server_url: String,
    /// HTTP esplora endpoint backing the on-chain wallet + chain queries.
    pub esplora_url: String,
    /// BIP-39 mnemonic the Arkade identity is derived from. MUST
    /// match the mnemonic used to construct the receive address
    /// passed to `create_swap` — otherwise the VHTLC's receiver
    /// keypair won't match the claim signer.
    pub identity_mnemonic: String,
    /// Bitcoin network the VHTLC sits on. Regtest for the local
    /// e2e; mainnet for production.
    pub network: BitcoinNetwork,
    /// Optional SQLite DB path for ark-rs contract-manager state.
    /// If omitted, ark-rs uses its default `contracts.db` path.
    pub contract_db_path: Option<String>,
}

impl From<ArkadeConfig> for SdkArkadeConfig {
    fn from(c: ArkadeConfig) -> Self {
        Self {
            arkade_server_url: c.arkade_server_url,
            esplora_url: c.esplora_url,
            identity_mnemonic: c.identity_mnemonic,
            network: c.network.into(),
            contract_db_path: c.contract_db_path.map(Into::into),
        }
    }
}

/// Result of an Arkade VHTLC claim.
#[derive(uniffi::Record, Clone, Debug)]
pub struct ClaimReceipt {
    /// Ark TX ID of the offchain claim transaction. Hex, `0x`-prefixed.
    pub ark_txid: String,
    pub claim_amount_sats: u64,
}

/// Offchain VTXO balance broken into the three buckets `ark-client`
/// distinguishes. Mirrors `lendaswap_sdk::arkade::ArkadeBalance`.
///
/// Offchain sends ([`SatoraClient::arkade_send`] and
/// [`SatoraClient::fund_arkade_swap`]) and offboards
/// ([`SatoraClient::arkade_offboard`], less VTXOs about to expire and the
/// dust it keeps as change) spend `confirmed_sats` and
/// `pre_confirmed_sats`. `recoverable_sats` moves only through
/// [`SatoraClient::arkade_settle`].
#[derive(uniffi::Record, Clone, Debug, Default, PartialEq, Eq)]
pub struct ArkadeBalance {
    pub confirmed_sats: u64,
    pub pre_confirmed_sats: u64,
    pub recoverable_sats: u64,
}

#[uniffi::export]
impl SatoraClient {
    /// Redeem the Arkade VHTLC for a Bitcoin, EVM or Lightning to Arkade
    /// swap that has reached (or passed) ServerFunded. Sweeps the BTC to
    /// `destination`. Requires a signing client; the Arkade identity
    /// mnemonic is provided separately via [`ArkadeConfig`] because
    /// it's distinct from the lendaswap signing mnemonic.
    ///
    /// A `Transport` or `Timeout` error leaves the outcome unknown, and so
    /// does `Storage` once arkd may hold the claim: call this again, which
    /// resumes from the journal. Once arkd has refused the stored claim
    /// because an input expired, the call fails with
    /// `SdkError::InvalidSwap`, and [`Self::settle`] then claims the VHTLC
    /// in a batch while the claim window is open.
    pub fn claim(&self, swap_id: String, destination: String) -> Result<ClaimReceipt, SdkError> {
        runtime().block_on(async {
            let receipt = self.inner.claim(&swap_id, &destination).await?;
            Ok(ClaimReceipt {
                ark_txid: format!("{:#x}", receipt.ark_txid),
                claim_amount_sats: receipt.claim_amount_sats,
            })
        })
    }

    /// Offchain VTXO balance of the SDK's internal Arkade wallet,
    /// broken into the three buckets ark-client distinguishes — see
    /// [`ArkadeBalance`]. Hits the Arkade server's gRPC indexer.
    pub fn arkade_balance(&self) -> Result<ArkadeBalance, SdkError> {
        runtime().block_on(async {
            let b = self.inner.arkade_balance().await?;
            Ok(ArkadeBalance {
                confirmed_sats: b.confirmed_sats,
                pre_confirmed_sats: b.pre_confirmed_sats,
                recoverable_sats: b.recoverable_sats,
            })
        })
    }

    /// Roll the wallet's recoverable VTXOs (expired, swept or below dust)
    /// and its boarding outputs into the next Arkade batch; healthy VTXOs
    /// are left alone. `None` => nothing to settle; `Some(txid)` => hex
    /// commitment txid (`0x…`) of the batch that absorbed them. This
    /// settles the wallet's own outputs; [`Self::settle`] spends a swap's
    /// VHTLC.
    ///
    /// A `Transport` error, or `Timeout` after 300 seconds, leaves the
    /// outcome unknown: the batch may still settle. Settling again is
    /// safe, since it only pays the wallet itself. The 300 seconds start
    /// once any wallet spend in progress has finished.
    pub fn arkade_settle(&self) -> Result<Option<String>, SdkError> {
        runtime().block_on(async {
            let txid = self.inner.arkade_settle().await?;
            Ok(txid.map(|t| format!("{:#x}", t)))
        })
    }

    /// Derive the SDK's internal Arkade wallet address. The same
    /// mnemonic always produces the same address (BIP-85 derivation),
    /// so this is the destination for funds from an address-less
    /// [`Self::create_swap`].
    pub fn arkade_offchain_address(&self) -> Result<String, SdkError> {
        runtime().block_on(async {
            self.inner
                .arkade_offchain_address()
                .await
                .map_err(Into::into)
        })
    }

    /// Send `amount_sats` from the SDK's internal Arkade wallet to
    /// `destination` (any `tark1q…` Arkade address) via an offchain
    /// Ark transaction. Returns the Ark txid as `0x…` hex. Fund a swap's
    /// VHTLC with [`Self::fund_arkade_swap`] instead, which journals the
    /// payment and resolves a retry from it.
    ///
    /// It spends confirmed and pre-confirmed VTXOs, including inputs a
    /// failed [`Self::fund_arkade_swap`] journaled; once arkd names and
    /// returns this spend, which pays nothing to its VHTLC, that call
    /// resolves its journal.
    ///
    /// A `Transport` error, or `Timeout` after 120 seconds, leaves the
    /// outcome unknown: arkd may have accepted the payment, so check the
    /// destination or the balance before sending again. The 120 seconds
    /// start once any wallet spend in progress has finished.
    pub fn arkade_send(&self, destination: String, amount_sats: u64) -> Result<String, SdkError> {
        runtime().block_on(async {
            let txid = self.inner.arkade_send(&destination, amount_sats).await?;
            Ok(format!("{:#x}", txid))
        })
    }

    /// On-chain Bitcoin boarding address for the SDK's internal
    /// Arkade wallet. Send L1 BTC here, then call
    /// [`Self::arkade_settle`] to promote the boarding output into a
    /// confirmed VTXO. The address is deterministic per wallet
    /// identity.
    pub fn arkade_boarding_address(&self) -> Result<String, SdkError> {
        runtime().block_on(async {
            self.inner
                .arkade_boarding_address()
                .await
                .map_err(Into::into)
        })
    }
}

// ─── Arkade settlement ─────────────────────────────────────────────────

/// Spend of a swap's Arkade VHTLC. Mirrors
/// `lendaswap_sdk::arkade::ArkadeSpend`.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArkadeSpend {
    /// Receiver path with the preimage: Bitcoin, EVM or Lightning to
    /// Arkade.
    Claim,
    /// Sender path once the VHTLC refund locktime has matured, without
    /// the swap server: Arkade to EVM or Lightning.
    Refund,
    /// Sender path cosigned by the swap server, before the locktime:
    /// Arkade to EVM or Lightning.
    CooperativeRefund,
}

impl From<ArkadeSpend> for SdkArkadeSpend {
    fn from(s: ArkadeSpend) -> Self {
        match s {
            ArkadeSpend::Claim => Self::Claim,
            ArkadeSpend::Refund => Self::Refund,
            ArkadeSpend::CooperativeRefund => Self::CooperativeRefund,
        }
    }
}

/// How [`SatoraClient::settle`] spent the VHTLC. Txids are hex,
/// `0x`-prefixed.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ArkadeSettlement {
    /// An offchain Ark transaction, as [`SatoraClient::claim`] submits.
    Offchain {
        ark_txid: String,
        claim_amount_sats: u64,
    },
    /// The VHTLC held recoverable outputs, which only a batch can spend:
    /// the commitment transaction of the batch that settled them.
    Delegated { commitment_txid: String },
}

impl From<SdkArkadeSettlement> for ArkadeSettlement {
    fn from(s: SdkArkadeSettlement) -> Self {
        match s {
            SdkArkadeSettlement::Offchain(receipt) => Self::Offchain {
                ark_txid: format!("{:#x}", receipt.ark_txid),
                claim_amount_sats: receipt.claim_amount_sats,
            },
            SdkArkadeSettlement::Delegated { commitment_txid } => Self::Delegated {
                commitment_txid: format!("{commitment_txid:#x}"),
            },
        }
    }
}

/// Sats at a swap's VHTLC by state. Mirrors
/// `lendaswap_sdk::arkade::VhtlcBalance` (TypeScript `VhtlcAmounts`).
#[derive(uniffi::Record, Clone, Debug, Default, PartialEq, Eq)]
pub struct VhtlcAmounts {
    /// Spendable by an offchain transaction.
    pub spendable_sats: u64,
    /// Expired outputs that only [`SatoraClient::settle`] can spend.
    pub recoverable_sats: u64,
    pub spent_sats: u64,
    /// Transactions that spent the VHTLC, hex, `0x`-prefixed; useful when
    /// another device completed the swap.
    pub spent_by: Vec<String>,
}

impl From<SdkVhtlcBalance> for VhtlcAmounts {
    fn from(b: SdkVhtlcBalance) -> Self {
        Self {
            spendable_sats: b.spendable_sats,
            recoverable_sats: b.recoverable_sats,
            spent_sats: b.spent_sats,
            spent_by: b.spent_by.iter().map(|t| format!("{t:#x}")).collect(),
        }
    }
}

#[uniffi::export]
impl SatoraClient {
    /// Claim or refund a swap's Arkade VHTLC to `destination`, an Arkade
    /// address. This spends a swap's VHTLC; [`Self::arkade_settle`]
    /// recovers the wallet's own expired, swept or sub-dust VTXOs and
    /// boarding outputs. Unlike [`Self::claim`], which refuses a VHTLC
    /// holding recoverable outputs, this settles those in a batch with the
    /// cosigner key the server reports. A claim is refused within 300
    /// seconds of the VHTLC refund locktime.
    ///
    /// A `Transport` or `Timeout` error leaves the outcome unknown, and so
    /// does `Storage` once arkd may hold the spend. The spend's progress is
    /// stored before submission, so calling this again with the same
    /// storage resumes it instead of spending twice; the swap status and
    /// [`Self::amounts_for_swap`] show where it stands.
    pub fn settle(
        &self,
        swap_id: String,
        operation: ArkadeSpend,
        destination: String,
    ) -> Result<ArkadeSettlement, SdkError> {
        runtime().block_on(async {
            let settlement = self
                .inner
                .settle(&swap_id, operation.into(), destination)
                .await?;
            Ok(settlement.into())
        })
    }

    /// Spendable, recoverable and spent sats at a stored swap's VHTLC,
    /// read through the internal Arkade wallet. Reads only the swap record
    /// in this client's storage, which swap creation and
    /// [`Self::recover_all_swaps`] write; a swap missing from it is
    /// `SdkError::InvalidSwap`.
    pub fn amounts_for_swap(&self, swap_id: String) -> Result<VhtlcAmounts, SdkError> {
        runtime().block_on(async {
            let amounts = self.inner.amounts_for_swap(&swap_id).await?;
            Ok(amounts.into())
        })
    }

    /// Whether a claim of a swap's Arkade VHTLC can still complete, read
    /// from the stored claim progress, the Arkade indexer and the claim
    /// window, without sending anything that carries the preimage. `false`
    /// is final: no later call can complete the claim. `true` means
    /// [`Self::settle`] finishes or resumes the claim, or already has.
    /// Reads only the swap record in this client's storage; a swap missing
    /// from it is `SdkError::InvalidSwap`.
    pub fn arkade_claim_possible(&self, swap_id: String) -> Result<bool, SdkError> {
        runtime().block_on(async { Ok(self.inner.arkade_claim_possible(&swap_id).await?) })
    }
}

// ─── Arkade wallet spends ──────────────────────────────────────────────

/// What [`SatoraClient::arkade_offboard`] pays on chain, and the server's
/// fee for it. Mirrors `lendaswap_sdk::arkade::OffboardQuote`.
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq, Eq)]
pub struct OffboardQuote {
    /// Sats the on-chain address receives.
    pub amount_sats: u64,
    /// The server's fee, taken from change on top of `amount_sats`.
    pub fee_sats: u64,
}

impl From<SdkOffboardQuote> for OffboardQuote {
    fn from(q: SdkOffboardQuote) -> Self {
        Self {
            amount_sats: q.amount_sats,
            fee_sats: q.fee_sats,
        }
    }
}

#[uniffi::export]
impl SatoraClient {
    /// Pay the VHTLC of an Arkade → Lightning or Arkade → EVM swap from
    /// the SDK's internal Arkade wallet and return the Ark txid as `0x…`
    /// hex. Wraps [`lendaswap_sdk::Client::fund_arkade_swap`]: the wallet
    /// inputs are journaled in this client's storage before the payment
    /// and the txid after it. Refuses with `SdkError::InvalidSwap` a swap
    /// that is not `Pending`, whose VHTLC holds outputs this client did not
    /// journal, or whose VHTLC is locked to a server key other than arkd's
    /// current signer (create a new swap then). Before journaling, it also
    /// refuses a VHTLC amount below arkd's dust, and a VHTLC whose hash
    /// lock is not the swap's: the hash of this client's preimage for
    /// Arkade → EVM, the payment hash of the stored invoice for Arkade →
    /// Lightning. A new selection draws on confirmed and pre-confirmed
    /// VTXOs. A swap this client never created or recovered fails with
    /// `SdkError::Storage`.
    ///
    /// A retry returns a journaled txid. Otherwise an Ark transaction arkd
    /// names as the spender of any journaled input may be the payment, or
    /// that of an earlier attempt still in flight when a retry widened the
    /// selection: the call returns that transaction once its VHTLC output
    /// is visible and fails with `SdkError::UnresolvedFunding` until then,
    /// unless arkd returns it and none of its outputs pays the VHTLC. A
    /// journaled input spent without arkd naming a spender, a journaled
    /// input arkd does not know, and a journal for a VHTLC address other
    /// than the swap's fail the same way and are never resolved
    /// automatically. While a funding is unresolved, follow the swap's
    /// status, never pay the VHTLC by other means, and give the swap up
    /// only once a check by hand shows where the funds went. Journaled
    /// inputs left unspent, forfeited into a batch, or spent by
    /// transactions that pay nothing to the VHTLC prove the payment never
    /// happened. The swap is then paid again under the checks above from a
    /// selection that includes every journaled input still spendable, so
    /// arkd accepts at most one of the two payments if the first is still
    /// in flight. When no journaled input is spendable any more, that rests
    /// on arkd also refusing the ones this client sees as expired, swept,
    /// unrolled or under a retired signer. A swap that no longer awaits
    /// funding instead has such a journal emptied before the refusal. With
    /// storage that kept this swap's journal, that refusal is the only
    /// error proving the wallet paid nothing; the others, such as a refusal
    /// before the journal is read, leave an earlier payment possible. A
    /// swap recovered on another device, or after a restart without
    /// `storage_path`, has no journal.
    ///
    /// A `Transport` error, or `Timeout` after 120 seconds, leaves the
    /// outcome unknown: call this again with the same storage, which
    /// resolves it from the journal, and never pay the VHTLC by other
    /// means such as [`Self::arkade_send`]. The 120 seconds start once any
    /// wallet spend in progress has finished. Once arkd accepted the
    /// payment, a failure to journal its txid is logged and the txid
    /// returned; a retry resolves it from the journaled inputs.
    pub fn fund_arkade_swap(&self, swap_id: String) -> Result<String, SdkError> {
        runtime().block_on(async {
            let txid = self.inner.fund_arkade_swap(&swap_id).await?;
            Ok(format!("{txid:#x}"))
        })
    }

    /// Send `amount_sats` from the SDK's internal Arkade wallet to the
    /// on-chain Bitcoin `address` in a collaborative exit and return the
    /// commitment txid as `0x…` hex. The server's fee comes out of change,
    /// so `address` receives exactly `amount_sats`. Every exit keeps at
    /// least the server's dust (330 sats today) in the wallet as Arkade
    /// change: ark-client 0.11 registers a signing key for every exit but
    /// signs the batch's VTXO tree only for one that creates a VTXO, so
    /// arkd would fail any batch an exit without change shares with
    /// others. `None` therefore sends the most that leaves at least dust as
    /// change (less than dust plus one fee step stays behind when the
    /// server's fee program skips the amount that leaves exactly dust), and
    /// a fixed amount that cannot leave dust is refused.
    ///
    /// The exit spends confirmed and pre-confirmed VTXOs only, the
    /// soonest-expiring first, so `None` sends everything except the
    /// recoverable balance and the change. Recoverable VTXOs (a batch that
    /// mixes them with healthy ones fails) and boarding outputs need
    /// [`Self::arkade_settle`] first; VTXOs carrying an asset stay
    /// offchain. VTXOs that expire before ark-client's four batch join
    /// attempts could end (each can wait out its intent's 120 s validity
    /// plus an arkd session; at most this call's 600 s) stay out too, since
    /// one that expires while the batch is pending fails it; once expired
    /// they are recoverable. On a server whose VTXOs live minutes, such as
    /// regtest's 512 s, that margin is half the VTXO lifetime, so a retried
    /// exit there can still meet an expiring input. The lifetime is read off
    /// the wallet's confirmed VTXOs, so a wallet that has only received
    /// offchain gets the full margin until a settle or boarding gives it one.
    ///
    /// Address, dust and balance checks fail with `SdkError::InvalidSwap`
    /// before anything is sent, naming the sats left out as expiring or
    /// recoverable when the spendable balance falls short, and so does a
    /// server fee program that charges the inputs or the change, which a
    /// collaborative exit does not pay. A `Transport` error, or `Timeout`
    /// after 600 seconds, leaves the outcome unknown: the batch may still
    /// settle, so check the balance before offboarding again. The 600
    /// seconds start once any wallet spend in progress has finished.
    pub fn arkade_offboard(
        &self,
        address: String,
        amount_sats: Option<u64>,
    ) -> Result<String, SdkError> {
        runtime().block_on(async {
            let txid = self.inner.arkade_offboard(&address, amount_sats).await?;
            Ok(format!("{txid:#x}"))
        })
    }

    /// The amount and fee [`Self::arkade_offboard`] would use, without
    /// sending. It waits for a wallet spend in progress on this client,
    /// so it prices what is left afterwards. It fails as
    /// `arkade_offboard` does, except that nothing is ever sent: call it
    /// again freely after a `Transport` error or a `Timeout` after 300
    /// seconds, which start once that spend has finished.
    pub fn arkade_offboard_quote(
        &self,
        address: String,
        amount_sats: Option<u64>,
    ) -> Result<OffboardQuote, SdkError> {
        runtime().block_on(async {
            let quote = self
                .inner
                .arkade_offboard_quote(&address, amount_sats)
                .await?;
            Ok(quote.into())
        })
    }
}

/// Reverse of the existing `From<SdkSwapStatus> for SwapStatus`. Needed
/// because `wait_for_swap_status` takes FFI-side `SwapStatus` values
/// for the targets array and we need to hand them to the SDK as
/// `SdkSwapStatus`. Kept as a free fn (rather than `impl From`) so
/// the existing one-way mapping stays unambiguous.
fn swap_status_into_sdk(s: SwapStatus) -> SdkSwapStatus {
    match s {
        SwapStatus::Pending => SdkSwapStatus::Pending,
        SwapStatus::ClientFundingSeen => SdkSwapStatus::ClientFundingSeen,
        SwapStatus::ClientFunded => SdkSwapStatus::ClientFunded,
        SwapStatus::ClientRefunded => SdkSwapStatus::ClientRefunded,
        SwapStatus::ServerFunded => SdkSwapStatus::ServerFunded,
        SwapStatus::ClientRedeeming => SdkSwapStatus::ClientRedeeming,
        SwapStatus::ClientRedeemed => SdkSwapStatus::ClientRedeemed,
        SwapStatus::ServerRedeemed => SdkSwapStatus::ServerRedeemed,
        SwapStatus::ClientFundedServerRefunded => SdkSwapStatus::ClientFundedServerRefunded,
        SwapStatus::ClientRefundedServerFunded => SdkSwapStatus::ClientRefundedServerFunded,
        SwapStatus::ClientRefundedServerRefunded => SdkSwapStatus::ClientRefundedServerRefunded,
        SwapStatus::Expired => SdkSwapStatus::Expired,
        SwapStatus::ClientInvalidFunded => SdkSwapStatus::ClientInvalidFunded,
        SwapStatus::ClientFundedTooLate => SdkSwapStatus::ClientFundedTooLate,
        SwapStatus::ServerWontFund => SdkSwapStatus::ServerWontFund,
        SwapStatus::ClientRedeemedAndClientRefunded => {
            SdkSwapStatus::ClientRedeemedAndClientRefunded
        }
        SwapStatus::Other { wire } => SdkSwapStatus::Other(wire),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lendaswap_sdk::types::GetSwapResponse;
    use lendaswap_sdk::types::RecoveredSwap;
    use serde_json::json;

    const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const OTHER_MNEMONIC: &str = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";
    const UNREACHABLE: &str = "http://127.0.0.1:1";
    const ARKADE_ADDRESS: &str = "tark1qqellv77udfmr20tun8dvju5vgudpf9vxe8jwhthrkn26fz96pawqfdy8nk05rsmrf8h94j26905e7n6sng8y059z8ykn2j5xcuw4xt846qj6x";
    const REGTEST_ADDRESS: &str = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";

    /// A server response from the Rust SDK's shared fixtures, with
    /// `overrides` applied.
    fn swap_response(direction: &str, overrides: serde_json::Value) -> GetSwapResponse {
        let mut fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../../rust-sdk/tests/fixtures/swap-responses.json"
        ))
        .unwrap();
        let mut response = fixtures[direction].take();
        for (field, value) in overrides.as_object().unwrap() {
            response[field] = value.clone();
        }
        serde_json::from_value(response).unwrap()
    }

    // Display order, as explorers show them. Every byte differs, so a
    // mapping that printed the internal byte order would not match.
    const TXID_A: &str = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
    const TXID_B: &str = "2122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f40";

    fn txid(display: &str) -> bitcoin::Txid {
        display.parse().unwrap()
    }

    fn client_with_storage(
        path: &std::path::Path,
        contract_db_path: Option<&std::path::Path>,
    ) -> Result<std::sync::Arc<SatoraClient>, SdkError> {
        let arkade_config = ArkadeConfig {
            arkade_server_url: UNREACHABLE.into(),
            esplora_url: UNREACHABLE.into(),
            identity_mnemonic: MNEMONIC.into(),
            network: BitcoinNetwork::Regtest,
            contract_db_path: contract_db_path.map(|p| p.to_string_lossy().into_owned()),
        };
        SatoraClient::new_with_arkade(
            UNREACHABLE.into(),
            MNEMONIC.into(),
            arkade_config,
            None,
            Some(path.to_string_lossy().into_owned()),
        )
    }

    #[test]
    fn every_sdk_error_keeps_its_variant_and_payload() {
        let message = || "detail".to_owned();
        let cases = [
            (
                SdkErrorInner::InvalidBaseUrl(message()),
                SdkError::InvalidBaseUrl { message: message() },
            ),
            (
                SdkErrorInner::Transport(message()),
                SdkError::Transport { message: message() },
            ),
            (
                SdkErrorInner::Decode(message()),
                SdkError::Decode { message: message() },
            ),
            (
                SdkErrorInner::Api {
                    status: 409,
                    message: message(),
                },
                SdkError::Api {
                    status: 409,
                    message: message(),
                },
            ),
            (
                SdkErrorInner::InvalidSigner(message()),
                SdkError::InvalidSigner { message: message() },
            ),
            (
                SdkErrorInner::InvalidSwap(message()),
                SdkError::InvalidSwap { message: message() },
            ),
            (
                SdkErrorInner::UnsupportedQuotePath(message()),
                SdkError::UnsupportedQuotePath { message: message() },
            ),
            (
                SdkErrorInner::Timeout(message()),
                SdkError::Timeout { message: message() },
            ),
            (
                SdkErrorInner::Storage(message()),
                SdkError::Storage { message: message() },
            ),
            (
                SdkErrorInner::UnresolvedFunding(message()),
                SdkError::UnresolvedFunding { message: message() },
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(SdkError::from(error), expected);
        }
        assert!(matches!(
            SdkError::from(SdkErrorInner::Cancelled),
            SdkError::Cancelled { message } if !message.is_empty()
        ));
    }

    #[test]
    fn unparseable_ffi_arguments_are_invalid_swap() {
        let opts = |node_rpc_url: Option<&str>, paymaster_context_json: Option<&str>| GaslessOpts {
            node_rpc_url: node_rpc_url.map(Into::into),
            paymaster_context_json: paymaster_context_json.map(Into::into),
            gas_overrides: None,
        };
        assert!(matches!(
            gasless_opts_into_sdk(opts(Some("not a url"), None)),
            Err(SdkError::InvalidSwap { .. })
        ));
        assert!(matches!(
            gasless_opts_into_sdk(opts(None, Some("{"))),
            Err(SdkError::InvalidSwap { .. })
        ));
        let client = SatoraClient::new(UNREACHABLE.into(), None).unwrap();
        assert!(matches!(
            client.check_deposit_funding("swap".into(), Some("not a url".into())),
            Err(SdkError::InvalidSwap { .. })
        ));
        assert!(matches!(
            client.create_swap(
                ChainId::Other { wire: "x".into() },
                TokenId::Btc,
                ChainId::Arkade,
                TokenId::Btc,
                QuoteAmount::Target { units: 1 },
                None,
                false,
                None,
            ),
            Err(SdkError::InvalidSwap { .. })
        ));
    }

    #[test]
    fn storage_path_opens_the_swap_store_at_construction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("swaps.sqlite");
        assert!(client_with_storage(&path, None).is_ok());
        assert!(path.exists());
    }

    #[test]
    fn storage_path_in_a_missing_directory_is_a_storage_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            client_with_storage(&dir.path().join("missing/swaps.sqlite"), None),
            Err(SdkError::Storage { .. })
        ));
    }

    #[test]
    fn contract_db_path_naming_the_swap_store_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let swaps = dir.path().join("swaps.sqlite");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let mut same_file = vec![swaps.clone(), dir.path().join("sub/../swaps.sqlite")];
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(dir.path(), &link).unwrap();
            same_file.push(link.join("swaps.sqlite"));
        }
        assert!(client_with_storage(&swaps, Some(&dir.path().join("contracts.sqlite"))).is_ok());
        // Only once the store exists can the file system say whether it
        // ignores letter case.
        let upper = dir.path().join("SWAPS.SQLITE");
        if upper.exists() {
            same_file.push(upper);
        }
        for contracts in same_file {
            assert!(
                matches!(
                    client_with_storage(&swaps, Some(&contracts)),
                    Err(SdkError::Storage { .. })
                ),
                "{}",
                contracts.display()
            );
        }
    }

    #[test]
    fn swap_funding_keeps_each_variant_and_its_payload() {
        // `UserSubmitted` is `#[non_exhaustive]` upstream and cannot be built
        // here, and every variant the SDK has is named, so no input reaches
        // `Unknown`.
        let cases = [
            (
                SdkSwapFunding::Gasless {
                    deposit_address: "0xdeposit".into(),
                },
                SwapFunding::Gasless {
                    deposit_address: "0xdeposit".into(),
                },
            ),
            (
                SdkSwapFunding::BitcoinAddress {
                    address: "bcrt1phtlc".into(),
                },
                SwapFunding::BitcoinAddress {
                    address: "bcrt1phtlc".into(),
                },
            ),
            (
                SdkSwapFunding::Bolt11Invoice {
                    invoice: "lnbcrt1invoice".into(),
                },
                SwapFunding::Bolt11Invoice {
                    invoice: "lnbcrt1invoice".into(),
                },
            ),
            (
                SdkSwapFunding::ArkadeAddress {
                    address: "tark1vhtlc".into(),
                },
                SwapFunding::ArkadeAddress {
                    address: "tark1vhtlc".into(),
                },
            ),
        ];
        for (funding, expected) in cases {
            assert_eq!(SwapFunding::from(funding), expected);
        }
    }

    #[test]
    fn create_swap_routes_bitcoin_sources_to_the_dispatcher() {
        // The EVM path refuses a BTC source locally; the dispatcher accepts
        // it and only fails on dialling the server.
        let client = SatoraClient::new_signing(UNREACHABLE.into(), MNEMONIC.into(), None).unwrap();
        let result = client.create_swap(
            ChainId::Bitcoin,
            TokenId::Btc,
            ChainId::Arkade,
            TokenId::Btc,
            QuoteAmount::Target { units: 100_000 },
            Some(Address::Arkade {
                address: ARKADE_ADDRESS.into(),
            }),
            false,
            None,
        );
        assert!(
            matches!(result, Err(SdkError::Transport { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn bitcoin_refund_receipt_carries_the_explorer_txid_and_fee() {
        let transaction = bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![bitcoin::TxIn {
                witness: bitcoin::Witness::from_slice(&[[7u8; 64]]),
                ..bitcoin::TxIn::default()
            }],
            output: vec![bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(1_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            }],
        };
        let receipt = BitcoinRefundReceipt::from(SdkBitcoinSpendReceipt {
            transaction,
            fee: bitcoin::Amount::from_sat(321),
        });
        // The txid without the witness, in display order as explorers and
        // `bitcoin-cli` take it, not the `0x` form the Ark txids here use.
        assert_eq!(
            receipt,
            BitcoinRefundReceipt {
                txid: "e4a6e5298645a8d4276bf11dc1257805015e4a35414482d51f9bcdaf57c3badc".into(),
                fee_sats: 321
            }
        );
    }

    #[test]
    fn refund_bitcoin_refuses_a_client_without_arkade_config() {
        let refund = |client: &SatoraClient| {
            client.refund_bitcoin("missing".into(), REGTEST_ADDRESS.into(), None, 1_000)
        };
        let signing = SatoraClient::new_signing(UNREACHABLE.into(), MNEMONIC.into(), None).unwrap();
        let result = refund(&signing);
        assert!(
            matches!(&result, Err(SdkError::InvalidSwap { message }) if message.contains("new_with_arkade")),
            "{result:?}"
        );
        // With the config, the unknown swap is what gets refused.
        let dir = tempfile::tempdir().unwrap();
        let arkade = client_with_storage(&dir.path().join("swaps.sqlite"), None).unwrap();
        let result = refund(&arkade);
        assert!(
            matches!(&result, Err(SdkError::InvalidSwap { message }) if !message.contains("new_with_arkade")),
            "{result:?}"
        );
    }

    #[test]
    fn refund_bitcoin_rejects_an_overflowing_fee_rate() {
        let dir = tempfile::tempdir().unwrap();
        let client = client_with_storage(&dir.path().join("swaps.sqlite"), None).unwrap();
        let result = client.refund_bitcoin(
            "missing".into(),
            REGTEST_ADDRESS.into(),
            Some(u64::MAX),
            1_000,
        );
        assert!(
            matches!(&result, Err(SdkError::InvalidSwap { message }) if message.contains("fee_rate_sat_vb")),
            "{result:?}"
        );
    }

    #[test]
    fn swap_direction_keeps_each_variant() {
        // Every variant the SDK has is named, so no input reaches `Unknown`.
        let cases = [
            (SdkSwapDirection::BtcToArkade, SwapDirection::BtcToArkade),
            (SdkSwapDirection::BitcoinToEvm, SwapDirection::BitcoinToEvm),
            (SdkSwapDirection::ArkadeToEvm, SwapDirection::ArkadeToEvm),
            (
                SdkSwapDirection::LightningToEvm,
                SwapDirection::LightningToEvm,
            ),
            (SdkSwapDirection::EvmToBitcoin, SwapDirection::EvmToBitcoin),
            (
                SdkSwapDirection::EvmToLightning,
                SwapDirection::EvmToLightning,
            ),
            (SdkSwapDirection::EvmToArkade, SwapDirection::EvmToArkade),
            (
                SdkSwapDirection::LightningToArkade,
                SwapDirection::LightningToArkade,
            ),
            (
                SdkSwapDirection::ArkadeToLightning,
                SwapDirection::ArkadeToLightning,
            ),
        ];
        for (direction, expected) in cases {
            assert_eq!(SwapDirection::from(direction), expected);
        }
    }

    #[test]
    fn swap_carries_its_direction_creation_time_and_deadlines() {
        // No direction has all four deadlines, so two directions cover them,
        // each deadline with its own value.
        let bitcoin = Swap::from(SdkSwap::from(swap_response(
            "btc_to_arkade",
            json!({
                "created_at": "2026-09-01T00:00:07Z",
                "vhtlc_refund_locktime": 1_001,
                "btc_refund_locktime": 1_002,
            }),
        )));
        assert_eq!(bitcoin.direction, SwapDirection::BtcToArkade);
        assert_eq!(bitcoin.created_at, "2026-09-01T00:00:07Z");
        assert_eq!(
            (
                bitcoin.vhtlc_refund_locktime,
                bitcoin.btc_refund_locktime,
                bitcoin.evm_refund_locktime,
                bitcoin.invoice_expires_at,
            ),
            (Some(1_001), Some(1_002), None, None)
        );
        let lightning = Swap::from(SdkSwap::from(swap_response(
            "evm_to_lightning",
            json!({ "evm_refund_locktime": 1_003, "invoice_expires_at": 1_004 }),
        )));
        assert_eq!(lightning.direction, SwapDirection::EvmToLightning);
        assert_eq!(
            (
                lightning.vhtlc_refund_locktime,
                lightning.btc_refund_locktime,
                lightning.evm_refund_locktime,
                lightning.invoice_expires_at,
            ),
            (None, None, Some(1_003), Some(1_004))
        );
    }

    #[test]
    fn lightning_destination_keeps_each_variant_and_amount() {
        let cases = [
            (
                LightningDestination::Invoice {
                    invoice: "lnbcrt1invoice".into(),
                },
                SdkLightningDestination::Invoice("lnbcrt1invoice".into()),
            ),
            (
                LightningDestination::Address {
                    address: "alice@example.com".into(),
                    sats: 1_000,
                },
                SdkLightningDestination::Address {
                    address: "alice@example.com".into(),
                    sats: 1_000,
                },
            ),
            (
                LightningDestination::Lnurl {
                    lnurl: "lnurl1alice".into(),
                    sats: 2_000,
                },
                SdkLightningDestination::Lnurl {
                    lnurl: "lnurl1alice".into(),
                    sats: 2_000,
                },
            ),
            (
                LightningDestination::AddressSendMax {
                    address: "alice@example.com".into(),
                    amount: 3_000,
                },
                SdkLightningDestination::AddressSendMax {
                    address: "alice@example.com".into(),
                    amount: 3_000,
                },
            ),
            (
                LightningDestination::LnurlSendMax {
                    lnurl: "lnurl1alice".into(),
                    amount: 4_000,
                },
                SdkLightningDestination::LnurlSendMax {
                    lnurl: "lnurl1alice".into(),
                    amount: 4_000,
                },
            ),
        ];
        for (destination, expected) in cases {
            assert_eq!(SdkLightningDestination::from(destination), expected);
        }
    }

    #[test]
    fn lightning_send_quote_keeps_every_field() {
        let quote = LightningSendQuote::from(LightningSendQuoteResponse {
            exchange_rate: "1".into(),
            max_amount_sats: 1,
            min_amount_sats: 2,
            network_fee_sats: 3,
            protocol_fee_rate: 0.004,
            protocol_fee_sats: 5,
            source_amount: "6".into(),
            source_amount_sats: 7,
            target_amount_sats: 8,
        });
        assert_eq!(
            quote,
            LightningSendQuote {
                exchange_rate: "1".into(),
                max_amount_sats: 1,
                min_amount_sats: 2,
                network_fee_sats: 3,
                protocol_fee_rate: 0.004,
                protocol_fee_sats: 5,
                source_amount: "6".into(),
                source_amount_sats: 7,
                target_amount_sats: 8,
            }
        );
    }

    #[test]
    fn lightning_send_quote_asks_for_an_arkade_btc_source() {
        // A matching EVM source passes local validation too, so only the
        // request tells them apart: Arkade BTC is the one source sent
        // without `source_token`.
        use std::io::BufRead;
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(&stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut header = String::new();
            while reader.read_line(&mut header).unwrap() > 2 {
                header.clear();
            }
            (&stream)
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            request_line
        });
        let client = SatoraClient::new(base_url, None).unwrap();
        let result = client.get_lightning_send_quote(
            LightningDestination::AddressSendMax {
                address: "alice@example.com".into(),
                amount: 100_000,
            },
            None,
        );
        assert!(
            matches!(result, Err(SdkError::Api { status: 404, .. })),
            "{result:?}"
        );
        let request_line = server.join().unwrap();
        assert!(
            request_line.starts_with("GET /quote/lightning-send?"),
            "{request_line}"
        );
        assert!(
            request_line.contains("source_chain=Arkade"),
            "{request_line}"
        );
        assert!(!request_line.contains("source_token"), "{request_line}");
    }

    #[test]
    fn arkade_spend_keeps_each_operation() {
        let cases = [
            (ArkadeSpend::Claim, SdkArkadeSpend::Claim),
            (ArkadeSpend::Refund, SdkArkadeSpend::Refund),
            (
                ArkadeSpend::CooperativeRefund,
                SdkArkadeSpend::CooperativeRefund,
            ),
        ];
        for (operation, expected) in cases {
            assert_eq!(SdkArkadeSpend::from(operation), expected);
        }
    }

    #[test]
    fn arkade_settlement_keeps_its_path_and_prefixes_txids() {
        let offchain = ArkadeSettlement::from(SdkArkadeSettlement::Offchain(
            lendaswap_sdk::arkade::ClaimReceipt {
                ark_txid: txid(TXID_A),
                claim_amount_sats: 5_000,
                destination: ARKADE_ADDRESS.into(),
            },
        ));
        assert_eq!(
            offchain,
            ArkadeSettlement::Offchain {
                ark_txid: format!("0x{TXID_A}"),
                claim_amount_sats: 5_000,
            }
        );
        let delegated = ArkadeSettlement::from(SdkArkadeSettlement::Delegated {
            commitment_txid: txid(TXID_B),
        });
        assert_eq!(
            delegated,
            ArkadeSettlement::Delegated {
                commitment_txid: format!("0x{TXID_B}"),
            }
        );
    }

    #[test]
    fn vhtlc_amounts_keep_each_bucket_and_spending_txid() {
        let amounts = VhtlcAmounts::from(SdkVhtlcBalance {
            spendable_sats: 1,
            recoverable_sats: 2,
            spent_sats: 3,
            spent_by: vec![txid(TXID_A), txid(TXID_B)],
        });
        assert_eq!(
            amounts,
            VhtlcAmounts {
                spendable_sats: 1,
                recoverable_sats: 2,
                spent_sats: 3,
                spent_by: vec![format!("0x{TXID_A}"), format!("0x{TXID_B}")],
            }
        );
    }

    #[test]
    fn offboard_quote_keeps_amount_and_fee() {
        let quote = OffboardQuote::from(SdkOffboardQuote {
            amount_sats: 1,
            fee_sats: 2,
        });
        assert_eq!(
            quote,
            OffboardQuote {
                amount_sats: 1,
                fee_sats: 2,
            }
        );
    }

    #[test]
    fn arkade_offboard_refuses_a_client_without_arkade_config() {
        // Refused before any server is dialled.
        let client = SatoraClient::new_signing(UNREACHABLE.into(), MNEMONIC.into(), None).unwrap();
        let no_wallet = |message: &str| message.contains("arkade_config");
        let send = client.arkade_offboard(REGTEST_ADDRESS.into(), None);
        assert!(
            matches!(&send, Err(SdkError::InvalidSwap { message }) if no_wallet(message)),
            "{send:?}"
        );
        let quote = client.arkade_offboard_quote(REGTEST_ADDRESS.into(), Some(10_000));
        assert!(
            matches!(&quote, Err(SdkError::InvalidSwap { message }) if no_wallet(message)),
            "{quote:?}"
        );
    }

    #[test]
    fn arkade_claim_possible_reads_the_stored_swap_before_arkd() {
        let client = SatoraClient::new_signing(UNREACHABLE.into(), MNEMONIC.into(), None).unwrap();
        let result = client.arkade_claim_possible("missing".into());
        assert!(
            matches!(&result, Err(SdkError::InvalidSwap { message }) if message == "swap record missing"),
            "{result:?}"
        );
    }

    #[test]
    fn fund_arkade_swap_refuses_a_swap_this_client_does_not_know() {
        // Refused from storage before any server is dialled.
        let client = SatoraClient::new_signing(UNREACHABLE.into(), MNEMONIC.into(), None).unwrap();
        let result = client.fund_arkade_swap("missing".into());
        assert!(
            matches!(&result, Err(SdkError::Storage { message }) if message.contains("swap key index not found")),
            "{result:?}"
        );
    }

    #[test]
    fn bulk_status_queries_in_chunks_of_100_and_merges_them_in_order() {
        let ids: Vec<String> = (0..250).map(|i| format!("swap-{i}")).collect();
        // Every seventh ID is unknown to the fake server.
        let unknown = |id: &String| id.trim_start_matches("swap-").parse::<u32>().unwrap() % 7 == 0;
        let mut requests: Vec<Vec<String>> = Vec::new();
        let merged = runtime()
            .block_on(bulk_status_in_chunks(&ids, |chunk| {
                requests.push(chunk.to_vec());
                let (not_found, found): (Vec<_>, Vec<_>) =
                    chunk.iter().cloned().partition(|id| unknown(id));
                std::future::ready(Ok(BulkStatusResponse {
                    not_found,
                    statuses: found
                        .into_iter()
                        .map(|id| SdkSwapStatusEntry {
                            id,
                            status: SdkSwapStatus::ServerFunded,
                        })
                        .collect(),
                }))
            }))
            .unwrap();
        assert_eq!(
            requests.iter().map(Vec::len).collect::<Vec<_>>(),
            [100, 100, 50]
        );
        assert_eq!(requests.concat(), ids);
        assert_eq!(
            merged,
            BulkStatus {
                statuses: ids
                    .iter()
                    .filter(|id| !unknown(id))
                    .map(|id| SwapStatusEntry {
                        id: id.clone(),
                        status: SwapStatus::ServerFunded,
                    })
                    .collect(),
                not_found: ids.iter().filter(|id| unknown(id)).cloned().collect(),
            }
        );
    }

    #[test]
    fn bulk_status_of_no_ids_sends_no_request() {
        let mut requests = 0;
        let merged = runtime()
            .block_on(bulk_status_in_chunks(&[], |_| {
                requests += 1;
                std::future::ready(Ok(BulkStatusResponse {
                    not_found: vec![],
                    statuses: vec![],
                }))
            }))
            .unwrap();
        assert_eq!(requests, 0);
        assert_eq!(merged, BulkStatus::default());
    }

    #[test]
    fn recovery_result_lists_the_recovered_swaps_and_the_cursor() {
        let recovered = |index, id: &str| RecoveredSwap {
            index,
            swap: swap_response("lightning_to_arkade", json!({ "id": id })),
        };
        let result = RecoveryResult::from(RecoveryResponse {
            swaps: vec![recovered(3, "swap-a"), recovered(5, "swap-b")],
            highest_index: 5,
            next_index: 6,
            scanned_until: 25,
            complete: true,
        });
        assert_eq!(
            result,
            RecoveryResult {
                swap_ids: vec!["swap-a".into(), "swap-b".into()],
                next_index: 6,
                scanned_until: 25,
            }
        );
    }

    #[test]
    fn contract_store_id_is_stable_and_differs_per_wallet() {
        let id = contract_store_id(MNEMONIC.into()).unwrap();
        // Pinned: a different derivation would rename existing contract
        // stores and strand the contracts recorded in them.
        assert_eq!(id, "fced0fde0196982a");
        assert_ne!(contract_store_id(OTHER_MNEMONIC.into()).unwrap(), id);
        assert!(matches!(
            contract_store_id("not a mnemonic".into()),
            Err(SdkError::InvalidSigner { .. })
        ));
    }
}
