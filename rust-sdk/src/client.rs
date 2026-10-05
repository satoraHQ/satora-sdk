//! HTTP client for the Lendaswap API.
//!
//! Typed requests, wallet key derivation, recovery, and swap orchestration.
//! API calls carry the SDK client header and one protocol epoch header per component.
//!
//! ## Public swap surface
//!
//! - [`Client::create_swap`] is the generic entry point: callers describe the swap in terms of
//!   tokens + amount + receive address; the dispatcher validates the direction and routes to a
//!   direction-specific helper.
//! - [`Client::create_evm_to_arkade_swap`] is the direction-specific helper for EVM stablecoin →
//!   BTC on Arkade. It owns validation, signer derivation, and secret persistence; the wire-level
//!   request struct is internal.

use crate::error::Error;
use crate::error::Result;
use crate::logging::LogLevel;
use crate::logging::Logger;
use crate::logging::SdkLogger;
use crate::request::Endpoint;
use crate::request::PayloadKind;
use crate::signer::Signer;
use crate::storage::InMemorySwapStorage;
use crate::storage::SqliteWalletStorage;
use crate::storage::SwapStorage;
use crate::storage::WalletStorage;
use crate::types::Address;
use crate::types::ErrorResponse;
use crate::types::EvmToArkadeSwapResponse;
use crate::types::GetSwapResponse;
use crate::types::KnownChain;
use crate::types::QuoteAmount;
use crate::types::SwapDirection;
use crate::types::SwapStatus;
use crate::types::TokenId;
use crate::types::Version;
use crate::types::VersionRequest;
use reqwest::StatusCode;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

mod api;
mod creation;
mod lightning;
mod quotes;
mod storage;

#[cfg(feature = "gasless")]
pub(crate) use lightning::INVOICE_HASH_MISMATCH;
#[cfg(feature = "gasless")]
pub(crate) use lightning::invoice_payment_hash;
pub use quotes::DEFAULT_COMPOSE_SLIPPAGE_BPS;

/// Production Lendaswap API endpoint. Used by [`ClientBuilder`] when no
/// `.base_url(…)` override is set, and by callers who don't care to spell
/// the URL themselves.
pub const DEFAULT_BASE_URL: &str = "https://api.satora.io";

/// Creation attempts before giving up. After each collision the loop also skips ten
/// further indices, as the TypeScript client does. Only an exact hash_lock match comes
/// back as a collision, so reuse across the HASH160 and SHA256 formats is not detected
/// here.
const MAX_KEY_INDEX_GRIND_ATTEMPTS: u32 = 10;
/// reqwest defaults to no timeout, and several execution paths hold a lock
/// across API calls.
const API_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// A hung connect then fails as a connect error, which proves nothing was sent,
/// instead of running into an operation deadline.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
fn default_http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(API_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}
pub(crate) const CLIENT_AGENT_HEADER: &str = "X-Lendaswap-Client";
pub(crate) const CLIENT_AGENT: &str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
/// Protocol epoch this SDK speaks for each protocol component, sent on every
/// request. Mirrors the server's `protocol_compat::Component`.
pub(crate) const PROTOCOL_HEADERS: [(&str, &str); 5] = [
    ("x-satora-bitcoin-htlc-version", "1"),
    ("x-satora-arkade-vhtlc-version", "1"),
    ("x-satora-lightning-version", "1"),
    ("x-satora-evm-erc20-htlc-version", "1"),
    ("x-satora-evm-native-htlc-version", "1"),
];

/// Attach every component's protocol epoch to a request.
pub(crate) fn protocol_headers(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    PROTOCOL_HEADERS
        .iter()
        .fold(builder, |builder, (name, value)| {
            builder.header(*name, *value)
        })
}

/// Retry only the backend's explicit reused-preimage rejection.
fn is_preimage_collision(err: &Error) -> bool {
    matches!(err, Error::Api { message, .. }
        if message.contains("a swap with this preimage hash exists already"))
}

#[derive(Clone)]
pub struct Client {
    pub(crate) arkade_execution: Arc<tokio::sync::Mutex<()>>,
    pub(crate) bitcoin_execution: Arc<tokio::sync::Mutex<()>>,
    quote_cache: Arc<tokio::sync::Mutex<quotes::QuoteCache>>,
    #[cfg(feature = "gasless")]
    pub(crate) evm_execution: Arc<tokio::sync::Mutex<()>>,
    pub(crate) http: reqwest::Client,
    pub(crate) base_url: Url,
    /// Optional: only required by [`Self::create_swap`]. Constructors that
    /// don't set it leave this `None`, and `create_swap` returns
    /// [`Error::InvalidSigner`].
    pub(crate) signer: Option<Signer>,
    pub(crate) storage: Arc<dyn SwapStorage>,
    /// Records for swaps whose store keeps only key indices.
    pub(crate) process_records: Arc<InMemorySwapStorage>,
    /// Optional referral code attached to every swap this client creates.
    /// Set via [`ClientBuilder::referral_code`].
    pub(crate) referral_code: Option<String>,
    /// Optional Arkade-side config. Required by every method that talks
    /// to arkd (`claim`, `arkade_balance`, `arkade_settle`,
    /// `arkade_offchain_address`, and `create_swap` with `receive_to =
    /// None`).
    pub(crate) arkade_config: Option<crate::arkade::ArkadeConfig>,
    /// Cached, lazily-initialised Arkade wallet. Held on the Client so
    /// stateful bits — saved boarding-output descriptors (`new_boarding_output`
    /// writes to a per-wallet in-memory DB), the BDK chain index, and
    /// the gRPC connection — survive across Client method calls.
    /// Without this, a `boarding_address` call and a follow-up
    /// `settle` would each construct a fresh wallet with an empty
    /// DB, so settle would never see the boarding output the user
    /// just funded.
    pub(crate) arkade_wallet: Arc<tokio::sync::OnceCell<crate::arkade::ArkadeWallet>>,
    pub(crate) log: SdkLogger,
    pub(crate) bitcoin_depth: crate::observation::BitcoinDepth,
    #[cfg(feature = "gasless")]
    pub(crate) evm_readers: EvmReaders,
    /// Basis points of the created target.
    #[cfg(feature = "gasless")]
    pub(crate) shortfall_limit: Arc<std::sync::atomic::AtomicU32>,
}

#[cfg(feature = "gasless")]
pub(crate) type EvmReaders =
    Arc<std::sync::RwLock<std::collections::HashMap<u64, Arc<dyn crate::evm::EvmWallet>>>>;

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
            .field("signer", &self.signer)
            .field("storage", &"<dyn SwapStorage>")
            .finish()
    }
}

impl Client {
    /// Construct a basic client targeting `base_url`. The resulting client
    /// has no signer attached: queries work, but swap creation and recovery
    /// return [`Error::InvalidSigner`]. Use [`Self::builder`] to attach a signer.
    pub fn new(base_url: &str) -> Result<Self> {
        crate::crypto_init::ensure_default_provider_installed();
        let base_url = Url::parse(base_url)?;
        Ok(Self {
            http: default_http(),
            base_url,
            signer: None,
            storage: Arc::new(InMemorySwapStorage::new()),
            process_records: Arc::default(),
            referral_code: None,
            arkade_config: None,
            arkade_wallet: Arc::new(tokio::sync::OnceCell::new()),
            arkade_execution: Arc::default(),
            bitcoin_execution: Arc::default(),
            quote_cache: Arc::default(),
            #[cfg(feature = "gasless")]
            evm_execution: Arc::default(),
            log: SdkLogger::tracing(),
            bitcoin_depth: crate::observation::BitcoinDepth::new(None),
            #[cfg(feature = "gasless")]
            evm_readers: Arc::default(),
            #[cfg(feature = "gasless")]
            shortfall_limit: Arc::default(),
        })
    }

    /// Start a [`ClientBuilder`] — required when calling [`Self::create_swap`].
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// The mnemonic this client signs with, like the TypeScript `getMnemonic`.
    /// `None` for an xprv or query-only client. To load a different mnemonic,
    /// save it with [`WalletStorage::set_mnemonic`] and build a new client.
    pub fn mnemonic(&self) -> Option<&str> {
        self.signer.as_ref()?.mnemonic()
    }

    /// Attach (or replace) the referral code attached to every
    /// swap/quote originating from this client. Pass `None` (or an
    /// empty string) to clear it. Useful after [`Self::new`], which
    /// doesn't go through the builder.
    pub fn set_referral_code(&mut self, code: Option<String>) {
        self.referral_code = code.filter(|s| !s.is_empty());
    }

    /// Replace the event sink and level, as [`ClientBuilder::logger`] and
    /// [`ClientBuilder::log_level`] do. Useful after [`Self::new`].
    pub fn set_logger(&mut self, logger: Arc<dyn Logger>, level: LogLevel) {
        self.log = SdkLogger::new(logger, level);
    }

    /// Confirmations a server-funded Bitcoin HTLC needs before it is claimable,
    /// like the TypeScript `getBitcoinMinConfirmations`. `None` reads as 0.
    pub fn bitcoin_min_confirmations(&self) -> Option<u32> {
        self.bitcoin_depth.get()
    }

    /// At 0 the claim goes out against a mempool funding, trusting the funder not
    /// to replace it once the claim reveals the preimage. Every clone of this
    /// client shares the setting, so it takes effect on the next chain read of
    /// every swap its trackers watch, including swaps already tracked.
    pub fn set_bitcoin_min_confirmations(&self, confirmations: Option<u32>) {
        self.bitcoin_depth.set(confirmations);
    }

    /// Read EVM chain `reader.chain_id()` through `reader`, replacing any earlier
    /// reader for that chain. When the daemon funds an EVM to Arkade swap's VHTLC
    /// below the target it was created with, because the source converted to less
    /// than quoted, a claim within [`Self::set_conversion_shortfall_limit`] reads
    /// the swap's own EVM lock through this and accepts the VHTLC only if it covers
    /// that target less the lock's shortfall. Without a reader for the swap's chain
    /// such a claim is refused. Only reads are made, so an
    /// [`crate::evm::RpcWallet`] over a node of that chain serves with any key.
    /// Every clone of this client shares the readers.
    #[cfg(feature = "gasless")]
    pub fn set_evm_reader(&self, reader: Arc<dyn crate::evm::EvmWallet>) {
        self.evm_readers
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(reader.chain_id(), reader);
    }

    /// Accept an EVM to Arkade VHTLC funded up to `basis_points` hundredths of a
    /// percent below the target its swap was created with, when the swap's own EVM
    /// lock shows a conversion shortfall that large (see [`Self::set_evm_reader`]).
    /// The default of 0 refuses any VHTLC below that target. The shortfall is
    /// measured against the lock the server said to expect, which nothing else
    /// checks, so this limit is all that bounds how far a dishonest server can
    /// lower a payout. A merchant's payment tolerance for such payments is a
    /// natural value. Every clone of this client shares the limit.
    #[cfg(feature = "gasless")]
    pub fn set_conversion_shortfall_limit(&self, basis_points: u32) {
        self.shortfall_limit
            .store(basis_points, std::sync::atomic::Ordering::Relaxed);
    }

    #[cfg(feature = "gasless")]
    pub(crate) fn evm_reader(&self, chain_id: u64) -> Option<Arc<dyn crate::evm::EvmWallet>> {
        self.evm_readers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&chain_id)
            .cloned()
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
        let builder = self
            .http
            .request(E::METHOD, url)
            .header(CLIENT_AGENT_HEADER, CLIENT_AGENT);
        let builder = protocol_headers(builder);
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
        let resp = protocol_headers(self.http.get(url).header(CLIENT_AGENT_HEADER, CLIENT_AGENT))
            .send()
            .await?;
        tracing::debug!(status = %resp.status(), "response received");
        let resp = check_status(resp).await?;
        Ok(resp.text().await?)
    }

    /// `GET /version`.
    pub async fn version(&self) -> Result<Version> {
        self.send(VersionRequest).await
    }

    /// Create a swap. Thin dispatcher: validates the direction
    /// (`source` / `target` / `receive_to` combination) and routes to the
    /// direction-specific method below.
    ///
    /// `receive_to = None` means "send to the SDK's own Arkade wallet" —
    /// requires the Client to have been built with
    /// [`ClientBuilder::arkade_config`] so the SDK knows which Arkade
    /// network to derive the address against. The address-less variant
    /// only makes sense when `target` is BTC.
    ///
    /// `source_chain` is required (not inferred from `source`) because
    /// [`TokenId::Btc`] is rail-ambiguous — `Lightning` and `Bitcoin`
    /// both source BTC. For EVM-side stablecoin sources where the
    /// `TokenId` already carries chain info (`UsdcPolygon`, etc.) the
    /// two must agree; mismatches return [`Error::InvalidSwap`].
    ///
    /// Supports all nine API directions. Generic EVM targets and invoice payouts
    /// use [`Self::create_swap_request`] for explicit chains and destinations.
    pub async fn create_swap(
        &self,
        source_chain: KnownChain,
        source: TokenId,
        target: TokenId,
        amount: QuoteAmount,
        receive_to: Option<Address>,
        extra_fees_bps: Option<u16>,
    ) -> Result<Swap> {
        // For EVM-side stablecoins the `TokenId` carries the chain;
        // catch a mismatch with the caller-supplied `source_chain` early
        // so we don't silently route to the wrong direction.
        if let Some(token_chain) = source.chain()
            && token_chain != source_chain
        {
            return Err(Error::InvalidSwap(format!(
                "source_chain ({source_chain:?}) disagrees with source token's native chain ({token_chain:?}) — TokenId::{source:?} lives on {token_chain:?}",
            )));
        }

        // Resolve `None` receive_to to the SDK's internal Arkade wallet.
        let receive_to = match receive_to {
            Some(addr) => addr,
            None => {
                if !matches!(target, TokenId::Btc) {
                    return Err(Error::InvalidSwap(
                        "receive_to=None only supported when target=BTC (the SDK's internal wallet lives on Arkade)".into(),
                    ));
                }
                let wallet = self.arkade_wallet().await?;
                Address::Arkade(wallet.offchain_address().await?)
            }
        };

        use crate::types::EvmAsset;
        use crate::types::LightningDestination;
        use crate::types::SwapCreation;
        use crate::types::SwapOptions;
        let evm_source = source_chain.evm_chain_id().map(|chain_id| EvmAsset {
            chain_id,
            token: source.clone(),
        });
        if evm_source.is_none() && source != TokenId::Btc {
            return Err(Error::InvalidSwap(
                "Bitcoin rails require TokenId::Btc".into(),
            ));
        }
        let request = match (source_chain, target, receive_to) {
            (KnownChain::Bitcoin, TokenId::Btc, Address::Arkade(target_address)) => {
                let QuoteAmount::Target(sats) = amount else {
                    return Err(Error::InvalidSwap(
                        "Bitcoin→Arkade requires an exact target amount".into(),
                    ));
                };
                validate_sats(sats)?;
                SwapCreation::BitcoinToArkade {
                    sats_receive: sats as u64,
                    target_address,
                }
            }
            (KnownChain::Lightning, TokenId::Btc, Address::Arkade(target_address)) => {
                SwapCreation::LightningToArkade {
                    amount,
                    target_address,
                }
            }
            (_, TokenId::Btc, Address::Arkade(target_address)) if evm_source.is_some() => {
                SwapCreation::EvmToArkade {
                    source: evm_source
                        .ok_or_else(|| Error::InvalidSwap("missing EVM source".into()))?,
                    amount,
                    target_address,
                }
            }
            (_, TokenId::Btc, Address::Bitcoin(target_address)) if evm_source.is_some() => {
                SwapCreation::EvmToBitcoin {
                    source: evm_source
                        .ok_or_else(|| Error::InvalidSwap("missing EVM source".into()))?,
                    amount,
                    target_address: Some(target_address),
                }
            }
            (rail, target, Address::Evm(target_address)) if evm_source.is_none() => {
                let chain_id = target.chain().and_then(|chain| chain.evm_chain_id()).ok_or_else(|| Error::InvalidSwap("generic EVM targets require create_swap_request with an explicit chain ID".into()))?;
                let target = EvmAsset {
                    chain_id,
                    token: target,
                };
                match rail {
                    KnownChain::Bitcoin => SwapCreation::BitcoinToEvm {
                        target,
                        amount,
                        target_address,
                    },
                    KnownChain::Arkade => SwapCreation::ArkadeToEvm {
                        target,
                        amount,
                        target_address,
                    },
                    KnownChain::Lightning => SwapCreation::LightningToEvm {
                        target,
                        amount,
                        target_address,
                    },
                    _ => return Err(Error::InvalidSwap("unsupported Bitcoin rail".into())),
                }
            }
            (rail, TokenId::Btc, Address::Lightning(destination))
                if rail == KnownChain::Arkade || evm_source.is_some() =>
            {
                let is_address = destination.contains('@');
                let is_lnurl = destination.to_ascii_lowercase().starts_with("lnurl");
                if !is_address && !is_lnurl {
                    return Err(Error::InvalidSwap("use LightningDestination::Invoice or InvoiceExpecting via create_swap_request for amount-bearing invoices".into()));
                }
                let destination = match (amount, is_address) {
                    (QuoteAmount::Target(sats), true) => {
                        validate_sats(sats)?;
                        LightningDestination::Address {
                            address: destination,
                            sats: sats as u64,
                        }
                    }
                    (QuoteAmount::Target(sats), false) => {
                        validate_sats(sats)?;
                        LightningDestination::Lnurl {
                            lnurl: destination,
                            sats: sats as u64,
                        }
                    }
                    (QuoteAmount::Source(amount), true) => LightningDestination::AddressSendMax {
                        address: destination,
                        amount,
                    },
                    (QuoteAmount::Source(amount), false) => LightningDestination::LnurlSendMax {
                        lnurl: destination,
                        amount,
                    },
                };
                match evm_source {
                    Some(source) => SwapCreation::EvmToLightning {
                        source,
                        destination,
                    },
                    None => SwapCreation::ArkadeToLightning { destination },
                }
            }
            _ => {
                return Err(Error::InvalidSwap(
                    "unsupported swap direction or destination".into(),
                ));
            }
        };
        self.create_swap_request(
            request,
            SwapOptions {
                extra_fees_bps,
                ..Default::default()
            },
        )
        .await
    }

    /// Create an EVM stablecoin → BTC-on-Arkade swap.
    ///
    /// The SDK derives the hash-lock secret, EVM signing address, and
    /// recovery `user_id` from the configured [`Signer`]; the key index is
    /// persisted to [`SwapStorage`] keyed by the returned swap ID for
    /// later claim. The wire-level request struct stays internal — callers
    /// only see [`Swap`].
    ///
    /// Validates:
    /// - `source` is a token on a recognised EVM chain (Polygon / Arbitrum / Ethereum).
    /// - `receive_to` is [`Address::Arkade`].
    /// - A signer is attached to the client (see [`Client::builder`]).
    ///
    /// **Funding modes**
    ///
    /// `gasless = true` runs the gasless deposit flow: the user is given
    /// an EVM deposit address derived from the configured [`Signer`] (see
    /// [`Signer::derive_evm_key`]) and just sends tokens to it from any
    /// wallet. Calling `fund_swap_gasless` then relays the funds into the
    /// HTLC with Permit2 authorization. Paymaster sponsorship is optional;
    /// without it the account needs native gas funds.
    ///
    /// `gasless = false` requires a wallet transaction. With the `gasless`
    /// feature enabled, use `get_permit2_funding_data` to inspect the typed
    /// request or `fund_swap_with_wallet` to validate, sign and submit it under
    /// explicit `FundingOptions` limits. Creation itself never broadcasts.
    ///
    /// **Referral code**: attached to every swap from this client; set
    /// once on the builder via [`ClientBuilder::referral_code`].
    pub async fn create_evm_to_arkade_swap(
        &self,
        source: TokenId,
        amount: QuoteAmount,
        receive_to: Address,
        gasless: bool,
        extra_fees_bps: Option<u16>,
    ) -> Result<Swap> {
        let chain_id = source
            .chain()
            .and_then(|chain| chain.evm_chain_id())
            .ok_or_else(|| {
                Error::InvalidSwap(
                    "generic tokens require create_swap_request with an explicit chain ID".into(),
                )
            })?;
        let Address::Arkade(target_address) = receive_to else {
            return Err(Error::InvalidSwap(
                "EVM→Arkade requires an Arkade destination".into(),
            ));
        };
        self.create_swap_request(
            crate::types::SwapCreation::EvmToArkade {
                source: crate::types::EvmAsset {
                    chain_id,
                    token: source,
                },
                amount,
                target_address,
            },
            crate::types::SwapOptions {
                gasless: Some(gasless),
                extra_fees_bps,
                ..Default::default()
            },
        )
        .await
    }

    /// Create an on-chain Bitcoin → BTC-on-Arkade swap.
    ///
    /// The payer sends `deposit_amount` sats to the returned
    /// [`SwapFunding::BitcoinAddress`]. Once the server funds the Arkade
    /// VHTLC, claim it with [`Self::claim`]. If the swap does not complete, the
    /// deposit comes back through [`Self::refund_bitcoin`] once
    /// `btc_refund_locktime` has matured.
    ///
    /// `sats_receive` is the exact amount to receive on Arkade; the API takes
    /// no source amount for this direction.
    ///
    /// Here and through [`Self::create_swap`] alike, the HTLC is rebuilt from the
    /// client's keys and the response, as [`Self::bitcoin_htlc`] does.
    /// [`Error::InvalidSwap`] refuses a hash lock that is not HASH160 of this client's
    /// preimage, an unknown network, an unsupported script version or zero refund
    /// locktime, and a `btc_htlc_address` other than the rebuilt one; [`Error::Decode`]
    /// refuses a server key, hash lock or refund locktime that does not parse. Such a
    /// swap stays in storage for recovery; never fund its address.
    pub async fn create_bitcoin_to_arkade_swap(
        &self,
        sats_receive: u64,
        receive_to: Address,
        extra_fees_bps: Option<u16>,
    ) -> Result<Swap> {
        let Address::Arkade(target_address) = receive_to else {
            return Err(Error::InvalidSwap(
                "Bitcoin→Arkade requires an Arkade destination".into(),
            ));
        };
        self.create_swap_request(
            crate::types::SwapCreation::BitcoinToArkade {
                sats_receive,
                target_address,
            },
            crate::types::SwapOptions {
                extra_fees_bps,
                ..Default::default()
            },
        )
        .await
    }

    /// Create a Lightning → BTC-on-Arkade swap.
    ///
    /// The user pays the returned Bolt11 invoice over Lightning; the
    /// server (via Boltz) observes the payment, locks BTC in an Arkade
    /// VHTLC, and the user claims it via [`Self::claim`] using the
    /// preimage the SDK re-derives from the signer.
    ///
    /// Mirrors [`Self::create_evm_to_arkade_swap`]'s key-index grind so
    /// recovered wallets transparently skip used indices. The on-chain
    /// EVM-derived fields (EOA, chain id) aren't needed for the
    /// Lightning rail — the user funds via their own Lightning wallet,
    /// not an EVM EOA.
    ///
    /// `sats_receive` is what the user wants to *receive* on Arkade.
    /// The server quotes the corresponding Bolt11 amount internally and
    /// embeds it in the invoice.
    ///
    /// An invoice that does not pay the hash of this client's preimage is
    /// refused with [`Error::InvalidSwap`] before anything is stored, as
    /// [`Self::create_swap_request`] describes.
    pub async fn create_lightning_to_arkade_swap(
        &self,
        sats_receive: u64,
        receive_to: Address,
    ) -> Result<Swap> {
        self.create_lightning_to_arkade_swap_with_options(
            QuoteAmount::Target(sats_receive.into()),
            receive_to,
            None,
            None,
        )
        .await
    }

    /// Create a Lightning → Arkade swap with an exact source or target amount.
    /// `invoice_description` is at most 639 UTF-8 bytes; `None` uses the server default.
    pub async fn create_lightning_to_arkade_swap_with_options(
        &self,
        amount: QuoteAmount,
        receive_to: Address,
        extra_fees_bps: Option<u16>,
        invoice_description: Option<String>,
    ) -> Result<Swap> {
        let Address::Arkade(target_address) = receive_to else {
            return Err(Error::InvalidSwap(
                "Lightning→Arkade requires an Arkade destination".into(),
            ));
        };
        self.create_swap_request(
            crate::types::SwapCreation::LightningToArkade {
                amount,
                target_address,
            },
            crate::types::SwapOptions {
                extra_fees_bps,
                invoice_description,
                ..Default::default()
            },
        )
        .await
    }

    /// Create an Arkade → BTC-on-Lightning swap.
    ///
    /// The user funds an Arkade VHTLC; the server uses Boltz to pay the
    /// user-provided Lightning [`crate::types::LightningDestination`] and claims the
    /// VHTLC with the resulting preimage. **No client-side claim path**
    /// — once the user funds the returned [`SwapFunding::ArkadeAddress`],
    /// the server owns the rest of the flow.
    ///
    /// The destination can be a BOLT11 invoice (optionally with the payout it
    /// must carry), or a Lightning address (`user@host`) or raw LNURL with a
    /// target or send-max amount. The server resolves addresses and LNURLs
    /// using LNURL-pay; no LNURL resolver is bundled.
    ///
    /// Duplicate-invoice errors are not retried: the invoice determines the
    /// hash lock, so allocating a new wallet key cannot resolve a collision.
    ///
    /// [`Self::create_swap`] accepts Lightning addresses and LNURLs; invoices
    /// need this method or [`Self::create_swap_request`].
    ///
    /// A hash lock that is not the payment hash of the returned invoice, or a
    /// returned invoice other than the supplied one, is refused with
    /// [`Error::InvalidSwap`] before anything is stored, as
    /// [`Self::create_swap_request`] describes.
    pub async fn create_arkade_to_lightning_swap(
        &self,
        destination: crate::types::LightningDestination,
    ) -> Result<Swap> {
        self.create_arkade_to_lightning_swap_with_options(destination, None)
            .await
    }

    /// Create an Arkade → Lightning swap with a per-swap referral surcharge.
    pub async fn create_arkade_to_lightning_swap_with_options(
        &self,
        destination: crate::types::LightningDestination,
        extra_fees_bps: Option<u16>,
    ) -> Result<Swap> {
        self.create_swap_request(
            crate::types::SwapCreation::ArkadeToLightning { destination },
            crate::types::SwapOptions {
                extra_fees_bps,
                ..Default::default()
            },
        )
        .await
    }

    /// `GET /swap/{id}` — fetch the current state of a swap by id.
    ///
    /// Not routed through [`Self::send`] because the endpoint has a
    /// path parameter and [`Endpoint::PATH`] is a `&'static str`. The
    /// shape varies per direction (the server wraps it in a
    /// `direction`-tagged enum); the `From<GetSwapResponse>` projection
    /// for [`Swap`] dispatches on the variant.
    ///
    /// Checked as [`Self::get_swap_details`] checks it.
    #[tracing::instrument(name = "get_swap", skip_all, fields(%swap_id))]
    pub async fn get_swap(&self, swap_id: &str) -> Result<Swap> {
        let body = self.get_swap_details(swap_id).await?;
        Ok(Swap::from(body))
    }

    /// Private: fetch the raw backend response without projection. `get_swap` consumers
    /// see the trimmed [`Swap`]; internal callers (e.g. the Arkade claim flow) need access
    /// to fields the public projection drops (VHTLC pubkeys, locktimes, network, …) and
    /// must dispatch on the variant themselves. Unlike [`Self::get_swap_details`], it
    /// checks neither the returned id nor a Lightning swap's invoice.
    pub(crate) async fn fetch_swap_response(&self, swap_id: &str) -> Result<GetSwapResponse> {
        let mut url = self.url("swap/")?;
        url.path_segments_mut()
            .map_err(|_| Error::InvalidBaseUrl("URL cannot contain path segments".into()))?
            .pop_if_empty()
            .push(swap_id);
        let resp = self.api_request(reqwest::Method::GET, url).send().await?;
        let resp = check_status(resp).await?;
        Ok(resp.json::<GetSwapResponse>().await?)
    }

    /// Poll `GET /swap/{id}` until the swap reaches one of `targets`
    /// (returning the matched status) or `timeout` elapses (returning
    /// [`Error::Timeout`]). Polls at a fixed 3s interval — fast enough
    /// for local e2e (Anvil block time ~1s, Arkade ~5s) without
    /// hammering a remote backend.
    ///
    /// Doesn't model "failure terminal states" (`Expired`,
    /// `ServerWontFund`, …) — if you need to short-circuit on those,
    /// the caller can layer a [`Self::get_swap`] check around this or
    /// we can grow a `fail_targets` parameter when there's a concrete
    /// user. For now, terminal failures will just surface as a timeout.
    ///
    /// The timeout covers HTTP requests and time spent sleeping between polls.
    /// Include terminal failure statuses in `targets` to stop on those states.
    ///
    /// Each poll is checked as [`Self::get_swap_details`] checks it, and a response it
    /// refuses ends the wait with [`Error::InvalidSwap`].
    #[tracing::instrument(name = "wait_for_swap_status", skip_all, fields(%swap_id, ?targets, ?timeout))]
    pub async fn wait_for_swap_status(
        &self,
        swap_id: &str,
        targets: &[SwapStatus],
        timeout: Duration,
    ) -> Result<SwapStatus> {
        const POLL_INTERVAL: Duration = Duration::from_secs(3);
        if targets.is_empty() {
            return Err(Error::InvalidSwap(
                "at least one target status is required".into(),
            ));
        }
        let mut attempt = 0u32;
        let mut last_status = None;
        let poll = async {
            loop {
                attempt += 1;
                let swap = self.get_swap(swap_id).await?;
                tracing::debug!(attempt, status = ?swap.status, "wait_for_swap_status: poll");
                if targets.contains(&swap.status) {
                    tracing::info!(attempt, status = ?swap.status, "wait_for_swap_status: matched");
                    return Ok(swap.status);
                }
                last_status = Some(swap.status);
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        };
        tokio::time::timeout(timeout, poll).await.map_err(|_| {
            Error::Timeout(format!(
                "swap {swap_id} did not reach {targets:?} within {timeout:?} (last status: {last_status:?})"
            ))
        })?
    }

    pub(crate) fn api_request(&self, method: reqwest::Method, url: Url) -> reqwest::RequestBuilder {
        protocol_headers(
            self.http
                .request(method, url)
                .header(CLIENT_AGENT_HEADER, CLIENT_AGENT),
        )
    }

    /// Fetch all server-provided swap details, without the compact display projection.
    ///
    /// A response that names another swap than `swap_id`, or that changes the direction
    /// of a swap stored here, is refused with [`Error::InvalidSwap`], and so is one whose
    /// invoice no longer pays the hash lock of a Lightning swap stored here: never pay or
    /// fund the swap then. A swap stored only as its key index gets the Lightning invoice
    /// and hash-lock checks of [`Self::recover_swaps`]. [`Self::refresh_swap`] leaves the
    /// store unchanged when a response is refused.
    pub async fn get_swap_details(&self, swap_id: &str) -> Result<GetSwapResponse> {
        let response = self.fetch_swap_response(swap_id).await?;
        self.check_swap_snapshot(swap_id, &response)?;
        Ok(response)
    }

    fn url(&self, path: &str) -> Result<Url> {
        // API paths are rooted at the server origin.
        Ok(self.base_url.join(&format!("/{path}"))?)
    }
}

/// Arkade-side operations share a lazily connected wallet across client clones.
/// The configured identity mnemonic derives the Arkade wallet seed.
///
/// Set the config via [`ClientBuilder::arkade_config`] at build time;
/// these methods return [`Error::InvalidSwap`] when no config is set.
impl Client {
    /// Fetch the stored [`crate::arkade::ArkadeConfig`] or error with a
    /// uniform message. Internal helper so every arkade-touching method
    /// has the same error wording.
    pub(crate) fn arkade_config(&self) -> Result<&crate::arkade::ArkadeConfig> {
        self.arkade_config.as_ref().ok_or_else(|| Error::InvalidSwap(
            "Client was constructed without an arkade_config — use ClientBuilder::arkade_config(...) at build time".into(),
        ))
    }

    /// Get the shared, lazily-connected [`crate::arkade::ArkadeWallet`].
    /// First call does the gRPC handshake; subsequent calls reuse the
    /// same wallet so its in-memory state (boarding-output descriptors,
    /// BDK chain index) is preserved across method calls.
    pub(crate) async fn arkade_wallet(&self) -> Result<&crate::arkade::ArkadeWallet> {
        self.arkade_wallet
            .get_or_try_init(|| async {
                crate::arkade::ArkadeWallet::connect(self.arkade_config()?).await
            })
            .await
    }

    /// Offchain VTXO balance of the internal Arkade wallet, broken
    /// into confirmed / pre-confirmed / recoverable buckets — see
    /// [`crate::arkade::ArkadeBalance`]. The headline number a caller
    /// usually wants is `confirmed_sats` (what's actually spendable
    /// via [`Self::arkade_send`]); `total_sats()` adds the other two
    /// for "are funds in flight?" checks.
    pub async fn arkade_balance(&self) -> Result<crate::arkade::ArkadeBalance> {
        let wallet = self.arkade_wallet().await?;
        wallet.offchain_balance().await
    }

    /// Roll over all spendable VTXOs + boarding outputs into the next
    /// Arkade batch — the operation users perform before their VTXOs
    /// expire. Returns `Ok(None)` if there's nothing to settle, or
    /// `Ok(Some(commitment_txid))` for the batch that absorbed them.
    pub async fn arkade_settle(&self) -> Result<Option<bitcoin::Txid>> {
        let wallet = self.arkade_wallet().await?;
        wallet.settle().await
    }

    /// Derive the internal Arkade wallet's offchain address. Useful for
    /// the caller to know where funds from an address-less
    /// [`Self::create_swap`] will land, or to display a deposit address
    /// for arbitrary inbound BTC.
    pub async fn arkade_offchain_address(&self) -> Result<String> {
        let wallet = self.arkade_wallet().await?;
        wallet.offchain_address().await
    }

    /// Send `amount_sats` from the internal Arkade wallet to
    /// `destination` via an offchain Ark transaction. Returns the Ark
    /// txid. Primary use case: funding the Arkade VHTLC returned by
    /// [`Self::create_arkade_to_lightning_swap`].
    pub async fn arkade_send(&self, destination: &str, amount_sats: u64) -> Result<bitcoin::Txid> {
        let wallet = self.arkade_wallet().await?;
        wallet.send_offchain(destination, amount_sats).await
    }

    /// On-chain Bitcoin boarding address for the internal Arkade
    /// wallet. Funding flow: send L1 BTC here, mine a confirmation,
    /// then call [`Self::arkade_settle`] to promote the boarding
    /// output into a confirmed VTXO. Use this when you want to fund
    /// the SDK's Arkade wallet from a regular Bitcoin wallet rather
    /// than via a swap.
    pub async fn arkade_boarding_address(&self) -> Result<String> {
        let wallet = self.arkade_wallet().await?;
        wallet.boarding_address().await
    }
}

/// Builder for [`Client`].
///
/// Required: a `mnemonic`, an `xprv` or wallet storage. With wallet storage and
/// no seed, the stored mnemonic is used, or a new one is generated and saved.
/// A supplied mnemonic is saved to empty wallet storage. Every other field has
/// a default. `base_url` defaults to [`DEFAULT_BASE_URL`], `storage` to
/// [`InMemorySwapStorage`], `http` to a fresh `reqwest::Client`.
#[derive(Default)]
pub struct ClientBuilder {
    base_url: Option<String>,
    mnemonic: Option<String>,
    xprv: Option<String>,
    wallet_storage: Option<Arc<dyn WalletStorage>>,
    wallet_storage_path: Option<PathBuf>,
    storage: Option<Arc<dyn SwapStorage>>,
    storage_path: Option<PathBuf>,
    http: Option<reqwest::Client>,
    referral_code: Option<String>,
    arkade_config: Option<crate::arkade::ArkadeConfig>,
    logger: Option<Arc<dyn Logger>>,
    log_level: Option<LogLevel>,
    bitcoin_min_confirmations: Option<u32>,
}

impl ClientBuilder {
    /// Override the target server URL. Defaults to [`DEFAULT_BASE_URL`]
    /// (`https://api.satora.io`) when not called.
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// BIP-39 mnemonic phrase. Mutually exclusive with [`Self::xprv`].
    pub fn mnemonic(mut self, mnemonic: impl Into<String>) -> Self {
        self.mnemonic = Some(mnemonic.into());
        self
    }

    /// BIP-32 extended private key (base58check). Mutually exclusive with
    /// [`Self::mnemonic`] and wallet storage: an xprv is never persisted.
    pub fn xprv(mut self, xprv: impl Into<String>) -> Self {
        self.xprv = Some(xprv.into());
        self
    }

    /// Keep the mnemonic in `storage`, like the TypeScript `withSignerStorage`.
    /// Without [`Self::mnemonic`], the stored mnemonic is used, or a new one is
    /// generated and saved. A supplied mnemonic is saved when the storage is
    /// empty and refused when it holds a different one. Mutually exclusive with
    /// [`Self::wallet_storage_path`].
    pub fn wallet_storage(mut self, storage: Arc<dyn WalletStorage>) -> Self {
        self.wallet_storage = Some(storage);
        self
    }

    /// [`Self::wallet_storage`] backed by a [`SqliteWalletStorage`] file, which
    /// holds the mnemonic in plaintext. It must not be the
    /// [`Self::storage_path`] database, and its parent directory must exist.
    pub fn wallet_storage_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.wallet_storage_path = Some(path.into());
        self
    }

    /// Storage backend for swap key indices. Defaults to [`InMemorySwapStorage`].
    /// Mutually exclusive with [`Self::storage_path`].
    pub fn storage(mut self, storage: Arc<dyn SwapStorage>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Persist key indices in SQLite, automatically partitioned by signing wallet.
    /// The parent directory must exist. Mutually exclusive with [`Self::storage`].
    pub fn storage_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.storage_path = Some(path.into());
        self
    }

    /// Inject an existing `reqwest::Client` (custom timeouts, middleware,
    /// shared pool, …). Defaults to one with a 30 s request and 10 s connect
    /// timeout. Its timeout does not apply to endpoints that block until
    /// settlement: those use the operation's own deadline.
    pub fn http(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// Referral code attached to every swap created by this client. Set
    /// once at builder time so per-swap call sites don't have to repeat
    /// it. Omit (or pass an empty string) to opt out.
    pub fn referral_code(mut self, code: impl Into<String>) -> Self {
        let code = code.into();
        self.referral_code = if code.is_empty() { None } else { Some(code) };
        self
    }

    /// Arkade-side config. Required by every method that talks to arkd
    /// (`claim`, `arkade_balance`, `arkade_settle`,
    /// `arkade_offchain_address`, and `create_swap` with no
    /// `receive_to`). Omit if the client will only do EVM-side
    /// operations. A blank `identity_mnemonic` is filled with the client's
    /// mnemonic.
    pub fn arkade_config(mut self, config: crate::arkade::ArkadeConfig) -> Self {
        self.arkade_config = Some(config);
        self
    }

    /// Sink for the SDK's structured events (see [`crate::logging`]), replacing
    /// the default `tracing` sink. Silent until [`Self::log_level`] is set, as
    /// in TypeScript.
    pub fn logger(mut self, logger: Arc<dyn Logger>) -> Self {
        self.logger = Some(logger);
        self
    }

    /// Minimum level of SDK events. Without a custom logger it defaults to
    /// `Trace`, leaving filtering to the `tracing` subscriber; with one it
    /// defaults to `Silent`.
    pub fn log_level(mut self, level: LogLevel) -> Self {
        self.log_level = Some(level);
        self
    }

    /// Confirmations a server-funded Bitcoin HTLC needs before it is claimable,
    /// like the TypeScript `withBitcoinMinConfirmations`. Default 0: claim as
    /// soon as the funding reaches the mempool, which trusts the funder not to
    /// double-spend it (claiming publishes the preimage, so a funder that then
    /// replaced its funding could take both legs). Set 1 or more for a
    /// block-depth policy that does not rely on the funder's good behaviour.
    /// Change it later with [`Client::set_bitcoin_min_confirmations`].
    pub fn bitcoin_min_confirmations(mut self, confirmations: u32) -> Self {
        self.bitcoin_min_confirmations = Some(confirmations);
        self
    }

    pub fn build(self) -> Result<Client> {
        crate::crypto_init::ensure_default_provider_installed();
        let base_url = self
            .base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let base_url = Url::parse(&base_url)?;
        if self.mnemonic.is_some() && self.xprv.is_some() {
            return Err(Error::InvalidSigner(
                "ClientBuilder: mnemonic and xprv are mutually exclusive".to_string(),
            ));
        }
        if self.xprv.is_some()
            && (self.wallet_storage.is_some() || self.wallet_storage_path.is_some())
        {
            return Err(Error::InvalidSigner(
                "ClientBuilder: an xprv is never persisted; remove wallet_storage".to_string(),
            ));
        }
        if self.storage.is_some() && self.storage_path.is_some() {
            return Err(Error::Storage(
                "storage and storage_path are mutually exclusive".into(),
            ));
        }
        let wallet_storage: Option<Arc<dyn WalletStorage>> =
            match (self.wallet_storage, self.wallet_storage_path) {
                (Some(_), Some(_)) => {
                    return Err(Error::Storage(
                        "wallet_storage and wallet_storage_path are mutually exclusive".into(),
                    ));
                }
                (Some(storage), None) => Some(storage),
                (None, Some(path)) => {
                    if let Some(swaps) = &self.storage_path
                        && same_file(&path, swaps)
                    {
                        return Err(Error::Storage(
                            "wallet_storage_path must not be the storage_path database".into(),
                        ));
                    }
                    Some(Arc::new(SqliteWalletStorage::open(path)?))
                }
                (None, None) => None,
            };
        let signer = match (self.mnemonic, self.xprv, wallet_storage) {
            (Some(m), _, storage) => {
                let signer = Signer::from_mnemonic(m)?;
                match storage {
                    Some(storage) => stored_signer(storage.as_ref(), Some(&signer))?,
                    None => signer,
                }
            }
            (None, Some(x), _) => Signer::from_xprv(x)?,
            (None, None, Some(storage)) => stored_signer(storage.as_ref(), None)?,
            (None, None, None) => {
                return Err(Error::InvalidSigner(
                    "ClientBuilder: provide mnemonic, xprv or wallet_storage".to_string(),
                ));
            }
        };
        let storage: Arc<dyn SwapStorage> = match (self.storage, self.storage_path) {
            (Some(storage), _) => storage,
            (None, Some(path)) => Arc::new(crate::SqliteSwapStorage::open(
                path,
                &signer.derive_user_id_xpub()?.to_string(),
            )?),
            (None, None) => Arc::new(InMemorySwapStorage::new()),
        };
        let mut arkade_config = self.arkade_config;
        if let Some(config) = &mut arkade_config
            && config.identity_mnemonic.trim().is_empty()
            && let Some(mnemonic) = signer.mnemonic()
        {
            config.identity_mnemonic = mnemonic.to_owned();
        }
        let http = self.http.unwrap_or_else(default_http);
        let log = match self.logger {
            Some(logger) => SdkLogger::new(logger, self.log_level.unwrap_or(LogLevel::Silent)),
            None => SdkLogger::new(
                Arc::new(crate::logging::TracingLogger),
                self.log_level.unwrap_or(LogLevel::Trace),
            ),
        };
        Ok(Client {
            http,
            base_url,
            signer: Some(signer),
            storage,
            process_records: Arc::default(),
            referral_code: self.referral_code,
            arkade_config,
            arkade_wallet: Arc::new(tokio::sync::OnceCell::new()),
            arkade_execution: Arc::default(),
            bitcoin_execution: Arc::default(),
            quote_cache: Arc::default(),
            #[cfg(feature = "gasless")]
            evm_execution: Arc::default(),
            log,
            bitcoin_depth: crate::observation::BitcoinDepth::new(self.bitcoin_min_confirmations),
            #[cfg(feature = "gasless")]
            evm_readers: Arc::default(),
            #[cfg(feature = "gasless")]
            shortfall_limit: Arc::default(),
        })
    }
}

/// Resolve the mnemonic kept in `storage`. Empty storage saves `supplied`, or
/// a newly generated mnemonic; another process may save first, and then its
/// mnemonic is used. A stored mnemonic is never overwritten here: this SDK's
/// swap records hold no keys, so it is the only key material for its swaps.
fn stored_signer(storage: &dyn WalletStorage, supplied: Option<&Signer>) -> Result<Signer> {
    let supplied = supplied.and_then(Signer::mnemonic);
    let stored = match supplied {
        Some(mnemonic) => storage.get_or_init_mnemonic(mnemonic)?,
        None => match storage.get_mnemonic()?.filter(|m| !m.is_empty()) {
            Some(stored) => stored,
            None => {
                let generated = Signer::generate(12)?;
                let mnemonic = generated.mnemonic().ok_or_else(|| {
                    Error::InvalidSigner("generated signer has no mnemonic".into())
                })?;
                storage.get_or_init_mnemonic(mnemonic)?
            }
        },
    };
    let signer = Signer::from_mnemonic(stored).map_err(|e| {
        Error::InvalidSigner(format!("wallet storage holds an invalid mnemonic: {e}"))
    })?;
    if supplied.is_some_and(|supplied| Some(supplied) != signer.mnemonic()) {
        return Err(Error::InvalidSigner(
            "wallet storage holds a different mnemonic; replace it with \
             WalletStorage::set_mnemonic"
                .into(),
        ));
    }
    Ok(signer)
}

/// Whether two paths name the same file, including a file not created yet.
fn same_file(a: &Path, b: &Path) -> bool {
    let resolve = |path: &Path| {
        std::fs::canonicalize(path).ok().or_else(|| {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            Some(std::fs::canonicalize(parent).ok()?.join(path.file_name()?))
        })
    };
    a == b || resolve(a).is_some_and(|a| resolve(b) == Some(a))
}

/// Compact, user-facing view of a created swap. Carries the fields a
/// caller needs to display payment instructions to the user and to time
/// claims and refunds.
///
/// Locktimes and expiries are unix seconds, `None` when the direction has no
/// such leg. A locktime on the leg the client funds is when the client's
/// refund unlocks. On the leg the server funds, the server can refund from
/// then, so it is the client's deadline to claim. Bitcoin and Arkade locks
/// mature on median time past, which trails wall clock by about an hour, so a
/// refund at the locktime can still be refused; [`Client::claim`] refuses
/// within 300 seconds of the VHTLC locktime.
///
/// `#[non_exhaustive]` so we can add fields in a SemVer-minor release.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Swap {
    pub id: String,
    pub status: SwapStatus,
    pub direction: SwapDirection,
    /// Funding instructions — depends on whether the swap was created
    /// with `gasless = true` (Gasless variant) or `gasless = false`
    /// (UserSubmitted variant).
    pub funding: SwapFunding,
    /// Amount the user must deposit, in the smallest unit of
    /// `deposit_token`. String to preserve precision for large EVM
    /// token amounts.
    pub deposit_amount: String,
    pub deposit_token: TokenId,
    /// Where the user receives the target asset (their `receive_to`).
    pub receive_address: String,
    /// Amount the user will receive, in the smallest unit of `receive_token`.
    pub receive_amount: String,
    pub receive_token: TokenId,
    /// Server creation time as sent on the wire (RFC 3339).
    pub created_at: String,
    /// Arkade VHTLC refund locktime. Bitcoin, EVM or Lightning to Arkade: the
    /// claim deadline. Arkade to EVM or Lightning: when the client's refund
    /// unlocks.
    pub vhtlc_refund_locktime: Option<i64>,
    /// On-chain Bitcoin HTLC refund locktime. Bitcoin to Arkade or EVM: when
    /// the client's refund unlocks. EVM to Bitcoin: the claim deadline.
    pub btc_refund_locktime: Option<i64>,
    /// EVM HTLC refund locktime. EVM to Arkade, Bitcoin or Lightning: when the
    /// client's refund unlocks. Bitcoin, Arkade or Lightning to EVM: the claim
    /// deadline.
    pub evm_refund_locktime: Option<i64>,
    /// Expiry of the invoice the payer pays (Lightning to Arkade or EVM) or the
    /// server pays (Arkade or EVM to Lightning). `None` also when the server
    /// reports no expiry, which it sends as 0. A payer must pay before both
    /// this expiry and the claim cutoff on the server-funded leg
    /// (`vhtlc_refund_locktime` for Lightning to Arkade, `evm_refund_locktime`
    /// for Lightning to EVM); the server does not guarantee which comes first.
    pub invoice_expires_at: Option<i64>,
}

/// How the user has to fund the swap.
///
/// Both the enum and its variants are `#[non_exhaustive]`:
/// - new variants (e.g. a future "user submits a signed permit2 message") can land as a
///   SemVer-minor change;
/// - new fields on existing variants are likewise non-breaking.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SwapFunding {
    /// `gasless = true` flow: the user sends tokens to `deposit_address`
    /// from any wallet. The SDK relays the funds into the HTLC via a
    /// Permit2-signed transaction so the user never pays gas.
    Gasless {
        /// EVM depositor address recorded by the server. New locally owned
        /// Rust swaps derive a separate key at `m/83696968'/121923'/{index}'/60'`.
        /// Recovered records can use TypeScript BIP44 or an external wallet.
        deposit_address: String,
    },
    /// `gasless = false` flow: the user submits the funding transaction
    /// themselves, calling the HTLC coordinator with specific calldata.
    ///
    /// Use `fund_swap_with_wallet` with reviewed `FundingOptions` when the
    /// `gasless` feature is enabled. Funding calls and contract configuration
    /// are fetched separately and checked against those application limits.
    #[non_exhaustive]
    UserSubmitted {},
    /// On-chain Bitcoin HTLC address to fund.
    BitcoinAddress { address: String },
    /// Lightning → Arkade flow: the user must pay this Bolt11 invoice
    /// over Lightning. The server observes the payment and
    /// then funds the Arkade VHTLC; the user offchain-claims with the
    /// preimage via [`Client::claim`].
    Bolt11Invoice {
        /// Bolt11-encoded Lightning invoice (`lnbc…` / `lnbcrt…` /
        /// `lntb…` depending on network).
        invoice: String,
    },
    /// Arkade → Lightning flow: the user must fund this Arkade VHTLC
    /// address with `deposit_amount` sats. Once funded, the server
    /// pays the user's Lightning invoice and claims the
    /// VHTLC with the resulting preimage — no client-side claim
    /// needed.
    ArkadeAddress {
        /// Arkade VHTLC address (`tark1q…`) the user funds.
        address: String,
    },
}

impl From<GetSwapResponse> for Swap {
    /// Project a `GetSwapResponse` (direction-tagged) into the
    /// per-direction-agnostic public [`Swap`] surface.
    fn from(r: GetSwapResponse) -> Self {
        match r {
            GetSwapResponse::BtcToArkade(r) => Self {
                id: r.id,
                status: r.status,
                direction: SwapDirection::BtcToArkade,
                funding: SwapFunding::BitcoinAddress {
                    address: r.btc_htlc_address,
                },
                deposit_amount: r.source_amount,
                deposit_token: r.source_token.token_id,
                receive_address: r.target_arkade_address,
                receive_amount: r.target_amount,
                receive_token: r.target_token.token_id,
                created_at: r.created_at,
                vhtlc_refund_locktime: Some(r.vhtlc_refund_locktime),
                btc_refund_locktime: Some(r.btc_refund_locktime),
                evm_refund_locktime: None,
                invoice_expires_at: None,
            },
            GetSwapResponse::BitcoinToEvm(r) => Self {
                id: r.id,
                status: r.status,
                direction: SwapDirection::BitcoinToEvm,
                funding: SwapFunding::BitcoinAddress {
                    address: r.btc_htlc_address,
                },
                deposit_amount: r.source_amount,
                deposit_token: r.source_token.token_id,
                receive_address: r.target_evm_address.unwrap_or(r.client_evm_address),
                receive_amount: r.target_amount,
                receive_token: r.target_token.token_id,
                created_at: r.created_at,
                vhtlc_refund_locktime: None,
                btc_refund_locktime: Some(r.btc_refund_locktime),
                evm_refund_locktime: Some(r.evm_refund_locktime),
                invoice_expires_at: None,
            },
            GetSwapResponse::ArkadeToEvm(r) => Self {
                id: r.id,
                status: r.status,
                direction: SwapDirection::ArkadeToEvm,
                funding: SwapFunding::ArkadeAddress {
                    address: r.btc_vhtlc_address,
                },
                deposit_amount: r.source_amount,
                deposit_token: r.source_token.token_id,
                receive_address: r.target_evm_address.unwrap_or(r.client_evm_address),
                receive_amount: r.target_amount,
                receive_token: r.target_token.token_id,
                created_at: r.created_at,
                vhtlc_refund_locktime: Some(r.vhtlc_refund_locktime),
                btc_refund_locktime: None,
                evm_refund_locktime: Some(r.evm_refund_locktime),
                invoice_expires_at: None,
            },
            GetSwapResponse::LightningToEvm(r) => Self {
                id: r.id,
                status: r.status,
                direction: SwapDirection::LightningToEvm,
                funding: SwapFunding::Bolt11Invoice {
                    invoice: r.bolt11_invoice,
                },
                deposit_amount: r.source_amount,
                deposit_token: r.source_token.token_id,
                receive_address: r.target_evm_address.unwrap_or(r.client_evm_address),
                receive_amount: r.target_amount,
                receive_token: r.target_token.token_id,
                created_at: r.created_at,
                vhtlc_refund_locktime: None,
                btc_refund_locktime: None,
                evm_refund_locktime: Some(r.evm_refund_locktime),
                invoice_expires_at: r.invoice_expires_at.filter(|&t| t > 0),
            },
            GetSwapResponse::EvmToBitcoin(r) => Self {
                id: r.id,
                status: r.status,
                direction: SwapDirection::EvmToBitcoin,
                funding: if r.gasless {
                    SwapFunding::Gasless {
                        deposit_address: r.client_evm_address,
                    }
                } else {
                    SwapFunding::UserSubmitted {}
                },
                deposit_amount: r.source_amount,
                deposit_token: r.source_token.token_id,
                receive_address: r.target_btc_address.unwrap_or_default(),
                receive_amount: r.target_amount,
                receive_token: r.target_token.token_id,
                created_at: r.created_at,
                vhtlc_refund_locktime: None,
                btc_refund_locktime: Some(r.btc_refund_locktime),
                evm_refund_locktime: Some(r.evm_refund_locktime),
                invoice_expires_at: None,
            },
            GetSwapResponse::EvmToLightning(r) => Self {
                id: r.id,
                status: r.status,
                direction: SwapDirection::EvmToLightning,
                funding: if r.gasless {
                    SwapFunding::Gasless {
                        deposit_address: r.client_evm_address,
                    }
                } else {
                    SwapFunding::UserSubmitted {}
                },
                deposit_amount: r.source_amount,
                deposit_token: r.source_token.token_id,
                receive_address: r.client_lightning_invoice,
                receive_amount: r.target_amount,
                receive_token: r.target_token.token_id,
                created_at: r.created_at,
                vhtlc_refund_locktime: None,
                btc_refund_locktime: None,
                evm_refund_locktime: Some(r.evm_refund_locktime),
                invoice_expires_at: Some(r.invoice_expires_at).filter(|&t| t > 0),
            },
            GetSwapResponse::EvmToArkade(r) => Self::from_evm_response(r),
            GetSwapResponse::LightningToArkade(r) => Self::from_lightning_response(r),
            GetSwapResponse::ArkadeToLightning(r) => Self::from_arkade_to_lightning_response(r),
        }
    }
}

impl Swap {
    fn from_evm_response(r: EvmToArkadeSwapResponse) -> Self {
        // `client_evm_address` is the address the SDK derives from the
        // signer's EVM key and echoes back in the response. In the
        // gasless flow it's where the user deposits their tokens.
        let funding = if r.gasless {
            SwapFunding::Gasless {
                deposit_address: r.client_evm_address,
            }
        } else {
            SwapFunding::UserSubmitted {}
        };
        Self {
            id: r.id,
            status: r.status,
            direction: SwapDirection::EvmToArkade,
            funding,
            deposit_amount: r.source_amount,
            deposit_token: r.source_token.token_id,
            receive_address: r.target_arkade_address,
            receive_amount: r.target_amount,
            receive_token: r.target_token.token_id,
            created_at: r.created_at,
            vhtlc_refund_locktime: i64::try_from(r.vhtlc_refund_locktime).ok(),
            btc_refund_locktime: None,
            evm_refund_locktime: i64::try_from(r.evm_refund_locktime).ok(),
            invoice_expires_at: None,
        }
    }

    fn from_lightning_response(r: crate::types::LightningToArkadeSwapResponse) -> Self {
        Self {
            id: r.id,
            status: r.status,
            direction: SwapDirection::LightningToArkade,
            funding: SwapFunding::Bolt11Invoice {
                invoice: r.bolt11_invoice,
            },
            deposit_amount: r.source_amount,
            deposit_token: r.source_token.token_id,
            receive_address: r.target_arkade_address,
            receive_amount: r.target_amount,
            receive_token: r.target_token.token_id,
            created_at: r.created_at,
            vhtlc_refund_locktime: i64::try_from(r.vhtlc_refund_locktime).ok(),
            btc_refund_locktime: None,
            evm_refund_locktime: None,
            invoice_expires_at: Some(r.invoice_expires_at).filter(|&t| t > 0),
        }
    }

    fn from_arkade_to_lightning_response(r: crate::types::ArkadeToLightningSwapResponse) -> Self {
        Self {
            id: r.id,
            status: r.status,
            direction: SwapDirection::ArkadeToLightning,
            funding: SwapFunding::ArkadeAddress {
                address: r.arkade_vhtlc_address,
            },
            deposit_amount: r.source_amount,
            deposit_token: r.source_token.token_id,
            // The Lightning side has no on-chain address; surface the
            // resolved invoice instead so callers can display
            // "you'll pay this invoice" to the user.
            receive_address: r.client_lightning_invoice,
            receive_amount: r.target_amount,
            receive_token: r.target_token.token_id,
            created_at: r.created_at,
            vhtlc_refund_locktime: i64::try_from(r.vhtlc_refund_locktime).ok(),
            btc_refund_locktime: None,
            evm_refund_locktime: None,
            invoice_expires_at: Some(r.invoice_expires_at).filter(|&t| t > 0),
        }
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

pub(crate) async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response> {
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

fn validate_sats(value: u128) -> Result<()> {
    if value == 0 || value > i64::MAX as u128 {
        return Err(Error::InvalidSwap(
            "satoshi amount must be positive and fit in i64".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryWalletStorage;

    fn arkade_config(identity_mnemonic: &str) -> crate::arkade::ArkadeConfig {
        crate::arkade::ArkadeConfig {
            arkade_server_url: "http://127.0.0.1:1".into(),
            esplora_url: "http://127.0.0.1:1".into(),
            identity_mnemonic: identity_mnemonic.into(),
            network: bitcoin::Network::Regtest,
            contract_db_path: None,
        }
    }

    #[test]
    fn blank_arkade_identity_uses_the_client_mnemonic() {
        let client = Client::builder()
            .wallet_storage(InMemoryWalletStorage::shared())
            .arkade_config(arkade_config(" "))
            .build()
            .unwrap();
        let identity = client
            .arkade_config
            .as_ref()
            .unwrap()
            .identity_mnemonic
            .as_str();
        assert_eq!(Some(identity), client.mnemonic());

        let explicit = Signer::generate(12).unwrap();
        let explicit = explicit.mnemonic().unwrap();
        let client = Client::builder()
            .wallet_storage(InMemoryWalletStorage::shared())
            .arkade_config(arkade_config(explicit))
            .build()
            .unwrap();
        let identity = client
            .arkade_config
            .as_ref()
            .unwrap()
            .identity_mnemonic
            .as_str();
        assert_eq!(identity, explicit);
        assert_ne!(Some(identity), client.mnemonic());
    }

    #[test]
    fn the_bitcoin_depth_is_seeded_by_the_builder_and_shared_by_every_clone() {
        assert_eq!(
            Client::new("http://127.0.0.1:1")
                .unwrap()
                .bitcoin_min_confirmations(),
            None
        );
        let client = Client::builder()
            .wallet_storage(InMemoryWalletStorage::shared())
            .bitcoin_min_confirmations(1)
            .build()
            .unwrap();
        assert_eq!(client.bitcoin_min_confirmations(), Some(1));
        let held_by_a_tracker = client.clone();
        client.set_bitcoin_min_confirmations(Some(0));
        assert_eq!(held_by_a_tracker.bitcoin_min_confirmations(), Some(0));
        held_by_a_tracker.set_bitcoin_min_confirmations(None);
        assert_eq!(client.bitcoin_min_confirmations(), None);
    }
}
