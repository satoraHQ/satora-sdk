//! HTTP client for the Lendaswap API.
//!
//! Typed requests, wallet key derivation, recovery, and swap orchestration.
//! API calls carry SDK and server-version compatibility headers.
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
use crate::request::Endpoint;
use crate::request::PayloadKind;
use crate::signer::Signer;
use crate::storage::InMemorySwapStorage;
use crate::storage::SwapStorage;
use crate::types::Address;
use crate::types::ErrorResponse;
use crate::types::EvmToArkadeSwapResponse;
use crate::types::GetSwapResponse;
use crate::types::KnownChain;
use crate::types::QuoteAmount;
use crate::types::QuoteRequest;
use crate::types::QuoteResponse;
use crate::types::SwapStatus;
use crate::types::TokenId;
use crate::types::Version;
use crate::types::VersionRequest;
use reqwest::StatusCode;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

mod api;
mod creation;
mod lightning;
mod quotes;
mod storage;

/// Production Lendaswap API endpoint. Used by [`ClientBuilder`] when no
/// `.base_url(…)` override is set, and by callers who don't care to spell
/// the URL themselves.
pub const DEFAULT_BASE_URL: &str = "https://api.satora.io";

/// Match the TypeScript client's bounded collision recovery policy.
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
const CLIENT_AGENT_HEADER: &str = "X-Lendaswap-Client";
const SATORA_SERVER_VERSION_HEADER: &str = "x-satora-server-version";
const CLIENT_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const SATORA_SERVER_VERSION: &str = env!("SATORA_SERVER_VERSION");

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
}

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
            referral_code: None,
            arkade_config: None,
            arkade_wallet: Arc::new(tokio::sync::OnceCell::new()),
            arkade_execution: Arc::default(),
            bitcoin_execution: Arc::default(),
            quote_cache: Arc::default(),
            #[cfg(feature = "gasless")]
            evm_execution: Arc::default(),
        })
    }

    /// Start a [`ClientBuilder`] — required when calling [`Self::create_swap`].
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Attach (or replace) the referral code attached to every
    /// swap/quote originating from this client. Pass `None` (or an
    /// empty string) to clear it. Useful after [`Self::new`], which
    /// doesn't go through the builder.
    pub fn set_referral_code(&mut self, code: Option<String>) {
        self.referral_code = code.filter(|s| !s.is_empty());
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
            .header(CLIENT_AGENT_HEADER, CLIENT_AGENT)
            .header(SATORA_SERVER_VERSION_HEADER, SATORA_SERVER_VERSION);
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
        let resp = self
            .http
            .get(url)
            .header(CLIENT_AGENT_HEADER, CLIENT_AGENT)
            .header(SATORA_SERVER_VERSION_HEADER, SATORA_SERVER_VERSION)
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

    /// `GET /quote` — fetch an exchange quote.
    pub async fn get_quote(&self, mut req: QuoteRequest) -> Result<QuoteResponse> {
        if req.referral_code.is_none() {
            req.referral_code = self.referral_code.clone();
        }
        self.send(req).await
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
                    return Err(Error::InvalidSwap("use a LightningDestination::Invoice via create_swap_request for amount-bearing invoices".into()));
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
    /// The destination can be a BOLT11 invoice (amount embedded), a
    /// Lightning address (`user@host`), or a raw LNURL — the server
    /// resolves the latter two using LNURL-pay. The SDK passes the
    /// caller's choice through verbatim; no LNURL resolver is bundled.
    ///
    /// Duplicate-invoice errors are not retried: the invoice determines the
    /// hash lock, so allocating a new wallet key cannot resolve a collision.
    ///
    /// Not reachable through the [`Self::create_swap`] dispatcher: the
    /// destination shape (3-variant input + sometimes-needed `sats`)
    /// doesn't fit cleanly into the dispatcher's `Address` +
    /// `QuoteAmount` arguments.
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
    /// `direction`-tagged enum); the projection in `Swap::from_response`
    /// dispatches on the variant.
    #[tracing::instrument(name = "get_swap", skip_all, fields(%swap_id))]
    pub async fn get_swap(&self, swap_id: &str) -> Result<Swap> {
        let body = self.fetch_swap_response(swap_id).await?;
        Ok(Swap::from_response(body))
    }

    /// Private: fetch the raw backend response without projection.
    /// `get_swap` consumers see the trimmed [`Swap`]; internal callers
    /// (e.g. the Arkade claim flow) need access to fields the public
    /// projection drops (VHTLC pubkeys, locktimes, network, …) and
    /// must dispatch on the variant themselves.
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
        self.http
            .request(method, url)
            .header(CLIENT_AGENT_HEADER, CLIENT_AGENT)
            .header(SATORA_SERVER_VERSION_HEADER, SATORA_SERVER_VERSION)
    }

    /// Fetch all server-provided swap details, without the compact display projection.
    pub async fn get_swap_details(&self, swap_id: &str) -> Result<GetSwapResponse> {
        self.fetch_swap_response(swap_id).await
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
/// Required: exactly one of `mnemonic` / `xprv` — every other field has a
/// default. `base_url` defaults to [`DEFAULT_BASE_URL`], `storage` to
/// [`InMemorySwapStorage`], `http` to a fresh `reqwest::Client`.
#[derive(Default)]
pub struct ClientBuilder {
    base_url: Option<String>,
    mnemonic: Option<String>,
    xprv: Option<String>,
    storage: Option<Arc<dyn SwapStorage>>,
    storage_path: Option<PathBuf>,
    http: Option<reqwest::Client>,
    referral_code: Option<String>,
    arkade_config: Option<crate::arkade::ArkadeConfig>,
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
    /// [`Self::mnemonic`].
    pub fn xprv(mut self, xprv: impl Into<String>) -> Self {
        self.xprv = Some(xprv.into());
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
    /// (`claim`, `arkade_balance_sats`, `arkade_settle`,
    /// `arkade_offchain_address`, and `create_swap` with no
    /// `receive_to`). Omit if the client will only do EVM-side
    /// operations.
    pub fn arkade_config(mut self, config: crate::arkade::ArkadeConfig) -> Self {
        self.arkade_config = Some(config);
        self
    }

    pub fn build(self) -> Result<Client> {
        crate::crypto_init::ensure_default_provider_installed();
        let base_url = self
            .base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let base_url = Url::parse(&base_url)?;
        let signer = match (self.mnemonic, self.xprv) {
            (Some(_), Some(_)) => {
                return Err(Error::InvalidSigner(
                    "ClientBuilder: mnemonic and xprv are mutually exclusive".to_string(),
                ));
            }
            (Some(m), None) => Signer::from_mnemonic(m)?,
            (None, Some(x)) => Signer::from_xprv(x)?,
            (None, None) => {
                return Err(Error::InvalidSigner(
                    "ClientBuilder: provide one of mnemonic or xprv".to_string(),
                ));
            }
        };
        let storage: Arc<dyn SwapStorage> = match (self.storage, self.storage_path) {
            (Some(_), Some(_)) => {
                return Err(Error::Storage(
                    "storage and storage_path are mutually exclusive".into(),
                ));
            }
            (Some(storage), None) => storage,
            (None, Some(path)) => Arc::new(crate::SqliteSwapStorage::open(
                path,
                &signer.derive_user_id_xpub()?.to_string(),
            )?),
            (None, None) => Arc::new(InMemorySwapStorage::new()),
        };
        let http = self.http.unwrap_or_else(default_http);
        Ok(Client {
            http,
            base_url,
            signer: Some(signer),
            storage,
            referral_code: self.referral_code,
            arkade_config: self.arkade_config,
            arkade_wallet: Arc::new(tokio::sync::OnceCell::new()),
            arkade_execution: Arc::default(),
            bitcoin_execution: Arc::default(),
            quote_cache: Arc::default(),
            #[cfg(feature = "gasless")]
            evm_execution: Arc::default(),
        })
    }
}

/// Compact, user-facing view of a created swap. Carries the fields a
/// caller needs to display payment instructions to the user.
///
/// `#[non_exhaustive]` so we can add fields (e.g. `expires_at`) in a
/// SemVer-minor release.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Swap {
    pub id: String,
    pub status: SwapStatus,
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

impl Swap {
    /// Project a `GetSwapResponse` (direction-tagged) into the
    /// per-direction-agnostic public [`Swap`] surface.
    fn from_response(r: GetSwapResponse) -> Self {
        match r {
            GetSwapResponse::BtcToArkade(r) => Self {
                id: r.id,
                status: r.status,
                funding: SwapFunding::BitcoinAddress {
                    address: r.btc_htlc_address,
                },
                deposit_amount: r.source_amount,
                deposit_token: r.source_token.token_id,
                receive_address: r.target_arkade_address,
                receive_amount: r.target_amount,
                receive_token: r.target_token.token_id,
            },
            GetSwapResponse::BitcoinToEvm(r) => Self {
                id: r.id,
                status: r.status,
                funding: SwapFunding::BitcoinAddress {
                    address: r.btc_htlc_address,
                },
                deposit_amount: r.source_amount,
                deposit_token: r.source_token.token_id,
                receive_address: r.target_evm_address.unwrap_or(r.client_evm_address),
                receive_amount: r.target_amount,
                receive_token: r.target_token.token_id,
            },
            GetSwapResponse::ArkadeToEvm(r) => Self {
                id: r.id,
                status: r.status,
                funding: SwapFunding::ArkadeAddress {
                    address: r.btc_vhtlc_address,
                },
                deposit_amount: r.source_amount,
                deposit_token: r.source_token.token_id,
                receive_address: r.target_evm_address.unwrap_or(r.client_evm_address),
                receive_amount: r.target_amount,
                receive_token: r.target_token.token_id,
            },
            GetSwapResponse::LightningToEvm(r) => Self {
                id: r.id,
                status: r.status,
                funding: SwapFunding::Bolt11Invoice {
                    invoice: r.bolt11_invoice,
                },
                deposit_amount: r.source_amount,
                deposit_token: r.source_token.token_id,
                receive_address: r.target_evm_address.unwrap_or(r.client_evm_address),
                receive_amount: r.target_amount,
                receive_token: r.target_token.token_id,
            },
            GetSwapResponse::EvmToBitcoin(r) => Self {
                id: r.id,
                status: r.status,
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
            },
            GetSwapResponse::EvmToLightning(r) => Self {
                id: r.id,
                status: r.status,
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
            },
            GetSwapResponse::EvmToArkade(r) => Self::from_evm_response(r),
            GetSwapResponse::LightningToArkade(r) => Self::from_lightning_response(r),
            GetSwapResponse::ArkadeToLightning(r) => Self::from_arkade_to_lightning_response(r),
        }
    }

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
            funding,
            deposit_amount: r.source_amount,
            deposit_token: r.source_token.token_id,
            receive_address: r.target_arkade_address,
            receive_amount: r.target_amount,
            receive_token: r.target_token.token_id,
        }
    }

    fn from_lightning_response(r: crate::types::LightningToArkadeSwapResponse) -> Self {
        Self {
            id: r.id,
            status: r.status,
            funding: SwapFunding::Bolt11Invoice {
                invoice: r.bolt11_invoice,
            },
            deposit_amount: r.source_amount,
            deposit_token: r.source_token.token_id,
            receive_address: r.target_arkade_address,
            receive_amount: r.target_amount,
            receive_token: r.target_token.token_id,
        }
    }

    fn from_arkade_to_lightning_response(r: crate::types::ArkadeToLightningSwapResponse) -> Self {
        Self {
            id: r.id,
            status: r.status,
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
