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
use lendaswap_sdk::aa::AaConfig as SdkAaConfig;
use lendaswap_sdk::aa::GasOverrides as SdkGasOverrides;
use lendaswap_sdk::aa::PaymasterConfig as SdkPaymasterConfig;
use lendaswap_sdk::aa::bundler::BundlerCasing as SdkBundlerCasing;
use lendaswap_sdk::arkade::ArkadeConfig as SdkArkadeConfig;
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

// ─── Gasless funding ───────────────────────────────────────────────────

/// Bundler RPC field-casing dialect. Mirrors `lendaswap_sdk::aa::bundler::BundlerCasing`.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BundlerCasing {
    /// Pimlico / ZeroDev — camelCase 7702-auth fields.
    CamelCase,
    /// Alchemy — snake_case 7702-auth fields.
    SnakeCase,
}

impl From<BundlerCasing> for SdkBundlerCasing {
    fn from(c: BundlerCasing) -> Self {
        match c {
            BundlerCasing::CamelCase => SdkBundlerCasing::CamelCase,
            BundlerCasing::SnakeCase => SdkBundlerCasing::SnakeCase,
        }
    }
}

/// Optional paymaster sponsorship. `context_json` is the paymaster-
/// specific context as a JSON string so it crosses the FFI boundary
/// cleanly (uniffi has no `serde_json::Value` type). For Alchemy Gas
/// Manager: `{"policyId":"<uuid>"}`. For paymasters that don't take
/// a context, pass `"null"`.
#[derive(uniffi::Record, Clone, Debug)]
pub struct PaymasterConfig {
    pub url: String,
    pub context_json: String,
}

/// AA / gasless-funding configuration. URL strings rather than typed
/// `Url` since uniffi doesn't bridge the alloy/url type. Invalid URLs
/// produce an `SdkError::Internal` from the SDK at construction time.
#[derive(uniffi::Record, Clone, Debug)]
pub struct AaConfig {
    pub bundler_url: String,
    pub node_rpc_url: String,
    /// `None` (default) means the depositor EOA pays its own gas;
    /// `Some(...)` enables a real paymaster to sponsor the userOp.
    pub paymaster: Option<PaymasterConfig>,
    /// Bundler casing default mirrors the SDK's: CamelCase works for
    /// Pimlico + ZeroDev (and any bundler accepting camelCase).
    pub bundler_casing: BundlerCasing,
    /// `Some(...)` to skip `eth_estimateUserOperationGas` and use
    /// the provided limits directly. Workaround for bundlers (alto
    /// in particular) that intermittently mis-simulate the userOp.
    pub gas_overrides: Option<GasOverrides>,
}

/// Explicit gas limits for the userOp. When set on [`AaConfig`], the
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

/// Submitted-userOp receipt. `transaction_hash` is `None` when the
/// SDK's bounded receipt poll ran out — callers can re-poll via
/// the bundler directly if needed.
#[derive(uniffi::Record, Clone, Debug)]
pub struct FundSwapReceipt {
    pub user_op_hash: String,
    pub transaction_hash: Option<String>,
}

/// Translate the FFI's URL-string config into the SDK's typed shape.
/// Returns `SdkError::Internal` for malformed URLs / JSON.
fn aa_config_into_sdk(c: AaConfig) -> Result<SdkAaConfig, SdkError> {
    use url::Url;
    let bundler_url = Url::parse(&c.bundler_url).map_err(|e| SdkError::Internal {
        message: format!("AaConfig.bundler_url parse: {e}"),
    })?;
    let node_rpc_url = Url::parse(&c.node_rpc_url).map_err(|e| SdkError::Internal {
        message: format!("AaConfig.node_rpc_url parse: {e}"),
    })?;
    let paymaster = c
        .paymaster
        .map(|pm| -> Result<SdkPaymasterConfig, SdkError> {
            let url = Url::parse(&pm.url).map_err(|e| SdkError::Internal {
                message: format!("PaymasterConfig.url parse: {e}"),
            })?;
            let context: serde_json::Value =
                serde_json::from_str(&pm.context_json).map_err(|e| SdkError::Internal {
                    message: format!("PaymasterConfig.context_json parse: {e}"),
                })?;
            Ok(SdkPaymasterConfig { url, context })
        })
        .transpose()?;
    Ok(SdkAaConfig {
        bundler_url,
        node_rpc_url,
        paymaster,
        bundler_casing: c.bundler_casing.into(),
        gas_overrides: c.gas_overrides.map(Into::into),
    })
}

#[uniffi::export]
impl LendaswapClient {
    /// Poll until the gasless deposit address holds enough source
    /// token AND enough native gas. Resolves the deposit address +
    /// required token amount from the swap response itself; the
    /// caller only supplies the gas headroom they want in wei.
    ///
    /// For an unsponsored userOp (no paymaster), ~0.001 ETH (= 1e15
    /// wei) is enough headroom on Arbitrum. With a paymaster, pass 0.
    ///
    /// Returns an `Internal` error wrapping the SDK's `Error::Timeout`
    /// if `timeout_seconds` elapses before both thresholds are met.
    pub fn wait_for_deposit_funding(
        &self,
        swap_id: String,
        aa_config: AaConfig,
        min_eth_wei: u64,
        timeout_seconds: u64,
    ) -> Result<(), SdkError> {
        let sdk_config = aa_config_into_sdk(aa_config)?;
        runtime().block_on(async {
            self.inner
                .wait_for_deposit_funding(
                    &swap_id,
                    &sdk_config,
                    min_eth_wei,
                    std::time::Duration::from_secs(timeout_seconds),
                )
                .await?;
            Ok(())
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
    pub fn fund_swap_gasless(
        &self,
        swap_id: String,
        aa_config: AaConfig,
    ) -> Result<FundSwapReceipt, SdkError> {
        let sdk_config = aa_config_into_sdk(aa_config)?;
        runtime().block_on(async {
            let receipt = self.inner.fund_swap_gasless(&swap_id, sdk_config).await?;
            Ok(FundSwapReceipt {
                user_op_hash: format!("{:#x}", receipt.user_op_hash),
                transaction_hash: receipt.transaction_hash.map(|h| format!("{h:#x}")),
            })
        })
    }
}

// ─── Status polling ────────────────────────────────────────────────────

#[uniffi::export]
impl LendaswapClient {
    /// `GET /swap/{id}` — fetch a swap's current state. Returns the
    /// same `Swap` shape `create_swap` does, so callers can re-read
    /// after the backend transitions states (e.g. ServerFunded).
    pub fn get_swap(&self, swap_id: String) -> Result<Swap, SdkError> {
        runtime().block_on(async {
            let swap = self.inner.get_swap(&swap_id).await?;
            Ok(swap.into())
        })
    }

    /// Poll `GET /swap/{id}` until the status matches one of `targets`
    /// or `timeout_seconds` elapses (returns `SdkError::Internal` with
    /// the SDK's `Error::Timeout` message in that case). 3s poll
    /// interval, fixed inside the SDK.
    ///
    /// `timeout_seconds: u64` because uniffi's `Duration` support is
    /// patchy across foreign bindgens; seconds is precise enough for
    /// swap-status polling (sub-second timeouts make no sense here).
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

/// Arkade-side configuration for [`LendaswapClient::claim`]. The
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
}

impl From<ArkadeConfig> for SdkArkadeConfig {
    fn from(c: ArkadeConfig) -> Self {
        Self {
            arkade_server_url: c.arkade_server_url,
            esplora_url: c.esplora_url,
            identity_mnemonic: c.identity_mnemonic,
            network: c.network.into(),
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

#[uniffi::export]
impl LendaswapClient {
    /// Redeem the Arkade VHTLC for an EVM→Arkade swap that has
    /// reached (or passed) ServerFunded. Sweeps the BTC to
    /// `destination`. Requires a signing client; the Arkade identity
    /// mnemonic is provided separately via [`ArkadeConfig`] because
    /// it's distinct from the lendaswap signing mnemonic.
    pub fn claim(
        &self,
        swap_id: String,
        destination: String,
        config: ArkadeConfig,
    ) -> Result<ClaimReceipt, SdkError> {
        runtime().block_on(async {
            let receipt = self
                .inner
                .claim(&swap_id, &destination, config.into())
                .await?;
            Ok(ClaimReceipt {
                ark_txid: format!("{:#x}", receipt.ark_txid),
                claim_amount_sats: receipt.claim_amount_sats,
            })
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
