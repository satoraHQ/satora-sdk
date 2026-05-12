//! HTTP client for the Lendaswap API.
//!
//! The public surface intentionally avoids generics and borrowed inputs so the
//! same shape can be re-exposed over a C-ABI / `csbindgen` / `interoptopus`
//! layer in a future crate. Generics only appear on the internal
//! [`Client::send`] helper, which is the single chokepoint every endpoint
//! flows through — that's where auth headers, retry, and tracing will plug in.
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
use crate::types::CreateEvmToArkadeSwapRequest;
use crate::types::ErrorResponse;
use crate::types::EvmToArkadeSwapResponse;
use crate::types::QuoteAmount;
use crate::types::QuoteRequest;
use crate::types::QuoteResponse;
use crate::types::SwapStatus;
use crate::types::TokenId;
use crate::types::Version;
use crate::types::VersionRequest;
use reqwest::StatusCode;
use serde::Serialize;
use std::sync::Arc;
use url::Url;

/// Production Lendaswap API endpoint. Used by [`ClientBuilder`] when no
/// `.base_url(…)` override is set, and by callers who don't care to spell
/// the URL themselves.
pub const DEFAULT_BASE_URL: &str = "https://api.satora.io";

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: Url,
    /// Optional: only required by [`Self::create_swap`]. Constructors that
    /// don't set it leave this `None`, and `create_swap` returns
    /// [`Error::InvalidSigner`].
    signer: Option<Signer>,
    storage: Arc<dyn SwapStorage>,
    /// Optional referral code attached to every swap this client creates.
    /// Set via [`ClientBuilder::referral_code`].
    referral_code: Option<String>,
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
    /// has no signer attached — low-level methods (`version`, `health`,
    /// `get_quote`, `create_evm_to_arkade_swap`) work, but `create_swap`
    /// will return [`Error::InvalidSigner`]. Use [`Self::builder`] for the
    /// high-level path.
    pub fn new(base_url: &str) -> Result<Self> {
        let base_url = Url::parse(base_url)?;
        Ok(Self {
            http: reqwest::Client::new(),
            base_url,
            signer: None,
            storage: Arc::new(InMemorySwapStorage::new()),
            referral_code: None,
        })
    }

    /// Start a [`ClientBuilder`] — required when calling [`Self::create_swap`].
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
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
        let builder = self.http.request(E::METHOD, url);
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
        let resp = self.http.get(url).send().await?;
        tracing::debug!(status = %resp.status(), "response received");
        let resp = check_status(resp).await?;
        Ok(resp.text().await?)
    }

    /// `GET /version`.
    pub async fn version(&self) -> Result<Version> {
        self.send(VersionRequest).await
    }

    /// `GET /quote` — fetch an exchange quote.
    pub async fn get_quote(&self, req: QuoteRequest) -> Result<QuoteResponse> {
        self.send(req).await
    }

    /// Create a swap. Thin dispatcher: validates the direction
    /// (`source` / `target` / `receive_to` combination) and routes to the
    /// direction-specific method below.
    ///
    /// Supported today: EVM stablecoin → BTC on Arkade. Any other
    /// combination returns [`Error::InvalidSwap`] — additional directions
    /// will land as separate `create_*_to_*_swap` methods.
    pub async fn create_swap(
        &self,
        source: TokenId,
        target: TokenId,
        amount: QuoteAmount,
        receive_to: Address,
    ) -> Result<Swap> {
        let source_is_evm = source.chain().and_then(|c| c.evm_chain_id()).is_some();
        let target_is_btc = matches!(target, TokenId::Btc);
        let target_is_arkade = matches!(&receive_to, Address::Arkade(_));

        if source_is_evm && target_is_btc && target_is_arkade {
            // Dispatcher defaults to non-gasless. Callers who need gasless
            // relay invoke `create_evm_to_arkade_swap` directly.
            return self
                .create_evm_to_arkade_swap(source, amount, receive_to, false)
                .await;
        }

        Err(Error::InvalidSwap(format!(
            "unsupported swap direction: {source:?} -> {target:?} (receive_to={receive_to:?}) — only EVM stablecoin -> BTC on Arkade is wired today",
        )))
    }

    /// Create an EVM stablecoin → BTC-on-Arkade swap.
    ///
    /// The SDK derives the hash-lock secret, EVM signing address, and
    /// recovery `user_id` from the configured [`Signer`]; the secret is
    /// persisted to [`SwapStorage`] keyed by the returned swap ID for
    /// later claim. The wire-level request struct stays internal — callers
    /// only see [`Swap`].
    ///
    /// Validates:
    /// - `source` is a token on a recognised EVM chain (Polygon / Arbitrum / Ethereum).
    /// - `receive_to` is [`Address::Arkade`].
    /// - A signer is attached to the client (see [`Client::builder`]).
    ///
    /// `gasless = true` asks the server to submit the funding tx on the
    /// user's behalf (Permit2 relay). The referral code attached to every
    /// swap from this client is set once on the builder via
    /// [`ClientBuilder::referral_code`].
    ///
    /// **Phase 1 caveat**: panics (`todo!()`) inside [`Signer`] — Phase 2
    /// wires the crypto in without changing this signature.
    pub async fn create_evm_to_arkade_swap(
        &self,
        source: TokenId,
        amount: QuoteAmount,
        receive_to: Address,
        gasless: bool,
    ) -> Result<Swap> {
        let source_chain = source.chain().ok_or_else(|| {
            Error::InvalidSwap(format!(
                "source token {source:?} has no known chain — pass a named EVM TokenId variant",
            ))
        })?;
        let evm_chain_id = source_chain.evm_chain_id().ok_or_else(|| {
            Error::InvalidSwap(format!(
                "source token {source:?} is not on an EVM chain (only Polygon / Ethereum / Arbitrum supported)",
            ))
        })?;
        let arkade_target_address = match &receive_to {
            Address::Arkade(s) => s.clone(),
            other => {
                return Err(Error::InvalidSwap(format!(
                    "EVM->Arkade swap requires an Arkade receive address, got {other:?}",
                )));
            }
        };

        let signer = self.signer.as_ref().ok_or_else(|| {
            Error::InvalidSigner(
                "Client constructed without a signer — use Client::builder() with .mnemonic / .xprv"
                    .to_string(),
            )
        })?;
        let key_index = 0; // Phase 2: pull from storage / increment per swap.
        let swap_params = signer.derive_swap_params(key_index)?;
        let evm_key = signer.derive_evm_key()?;
        let hash_lock = format!("0x{}", hex_encode(&swap_params.hash_lock));
        let receiver_pk = hex_encode(&swap_params.public_key);
        let user_id = hex_encode(&swap_params.user_id);

        let req = CreateEvmToArkadeSwapRequest::new(
            arkade_target_address,
            evm_chain_id,
            source,
            hash_lock,
            receiver_pk,
            evm_key.address,
            user_id,
            amount,
            gasless,
            self.referral_code.clone(),
        );
        let response = self.send(req).await?;

        // Persist the secret so we can claim later.
        self.storage.put_secret(&response.id, &swap_params.secret)?;

        Ok(Swap::from_response(response))
    }

    fn url(&self, path: &str) -> Result<Url> {
        // join() respects whether `base_url` ends in a slash; we always treat
        // `path` as a relative segment with no leading slash.
        Ok(self.base_url.join(&format!("/{path}"))?)
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
    http: Option<reqwest::Client>,
    referral_code: Option<String>,
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

    /// Storage backend for swap secrets. Defaults to [`InMemorySwapStorage`].
    pub fn storage(mut self, storage: Arc<dyn SwapStorage>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Inject an existing `reqwest::Client` (custom timeouts, middleware,
    /// shared pool, …). Defaults to a fresh one.
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

    pub fn build(self) -> Result<Client> {
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
        let storage = self
            .storage
            .unwrap_or_else(|| Arc::new(InMemorySwapStorage::new()) as Arc<dyn SwapStorage>);
        let http = self.http.unwrap_or_default();
        Ok(Client {
            http,
            base_url,
            signer: Some(signer),
            storage,
            referral_code: self.referral_code,
        })
    }
}

/// Compact, user-facing view of a created swap. Carries the fields a
/// caller needs to display payment instructions to the user.
///
/// The full backend response is reachable via the low-level
/// [`Client::create_evm_to_arkade_swap`] entry point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Swap {
    pub id: String,
    pub status: SwapStatus,
    /// Where the user must send their source funds.
    pub deposit_address: String,
    /// Amount the user must send, in the smallest unit of `deposit_token`.
    /// String to preserve precision for large EVM token amounts.
    pub deposit_amount: String,
    pub deposit_token: TokenId,
    /// Where the user receives the target asset (their `receive_to`).
    pub receive_address: String,
    /// Amount the user will receive, in the smallest unit of `receive_token`.
    pub receive_amount: String,
    pub receive_token: TokenId,
}

impl Swap {
    fn from_response(r: EvmToArkadeSwapResponse) -> Self {
        Self {
            id: r.id,
            status: r.status,
            deposit_address: r.evm_htlc_address,
            deposit_amount: r.source_amount,
            deposit_token: r.source_token.token_id,
            receive_address: r.target_arkade_address,
            receive_amount: r.target_amount,
            receive_token: r.target_token.token_id,
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
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

async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response> {
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
