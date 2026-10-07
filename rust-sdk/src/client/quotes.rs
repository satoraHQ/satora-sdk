use super::Client;
use super::check_status;
use crate::Error;
use crate::Result;
use crate::logging::sdk_log;
use crate::types::*;
use num_bigint::BigUint;
use num_traits::ToPrimitive;
use num_traits::Zero;
use reqwest::Method;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;
use std::time::Instant;

/// Slippage [`Client::get_quote`] composes with, the TypeScript SDK's default.
pub const DEFAULT_COMPOSE_SLIPPAGE_BPS: u16 = 100;

/// The TypeScript SDK's EVM chains (`ALL_EVM_CHAIN_IDS` in tokens.ts) other than
/// the Ethereum, Polygon and Arbitrum hubs. The daemon serves none of them; they
/// are CCTP or USDT0 bridge chains.
const BRIDGE_CHAIN_NAMES: [(&str, &str); 25] = [
    ("10", "Optimism"),
    ("8453", "Base"),
    ("43114", "Avalanche"),
    ("59144", "Linea"),
    ("130", "Unichain"),
    ("146", "Sonic"),
    ("480", "World Chain"),
    ("57073", "Ink"),
    ("1329", "Sei"),
    ("999", "HyperEVM"),
    ("143", "Monad"),
    ("50", "XDC"),
    ("80094", "Berachain"),
    ("1030", "Conflux eSpace"),
    ("21000000", "Corn"),
    ("14", "Flare"),
    ("295", "Hedera"),
    ("5000", "Mantle"),
    ("4326", "MegaETH"),
    ("2818", "Morph"),
    ("9745", "Plasma"),
    ("30", "Rootstock"),
    ("988", "Stable"),
    ("4217", "Tempo"),
    ("196", "XLayer"),
];

/// Native USDC on each CCTP chain outside the hubs (`CHAIN_ID_TO_CCTP_NAME` and
/// `USDC_ADDRESSES`).
const CCTP_ONLY_USDC: [(&str, &str); 12] = [
    ("10", "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85"),
    ("130", "0x078d782b760474a361dda0af3839290b0ef57ad6"),
    ("146", "0x29219dd400f2bf60e5a23d13be72b486d4038894"),
    ("1329", "0xe15fC38F6D8c56aF07bbCBe3BAf5708A2Bf42392"),
    ("8453", "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
    ("43114", "0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E"),
    ("59144", "0x176211869ca2b568f2a7d4ee941e073a821ee1ff"),
    ("480", "0x79A02482A880bCe3F13E09da970dC34dB4cD24D1"),
    ("57073", "0x2D270e6886d130D724215A266106e6832161EAEd"),
    ("999", "0xb88339CB7199b77E23DB6E890353E22632Ba630f"),
    ("143", "0x754704Bc059F8C67012fEd69BC8A327a5aafb603"),
    ("50", "0xfA2958CB79b0491CC627c1557F441eF849Ca8eb1"),
];

/// Every distinct USDT0 contract in `USDT0_ADDRESSES` (usdt0-bridge/constants.ts).
const USDT0_ADDRESSES: [&str; 12] = [
    "0xdAC17F958D2ee523a2206206994597C13D831ec7",
    "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9",
    "0xc2132D05D31c914a87C6611C10748AEb04B58e8F",
    "0x01bFF41798a0BcF287b996046Ca68b395DbC1071",
    "0x779Ded0c9e1022225f8E0630b35a9b54bE713736",
    "0xaf37E8B6C9ED7f6318979f56Fc287d76c30847ff",
    "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb",
    "0xe7cd86e13AC4309349F30B3435a9d337750fC82D",
    "0x00000000000000000000000000000000009Ce723",
    "0x0200C29006150606B650577BBE7B6248F58470c1",
    "0x9151434b16b9763660705744891fA906F660EcC5",
    "0x20C00000000000000000000014f22CA97301EB73",
];

#[derive(Default)]
pub(super) struct QuoteCache(HashMap<String, (Instant, Value)>);

fn invalid(message: &str) -> Error {
    Error::InvalidSwap(message.into())
}
fn unsupported(message: &str) -> Error {
    Error::UnsupportedQuotePath(message.into())
}
fn integer(value: &str) -> Result<BigUint> {
    if value.is_empty() || value.len() > 78 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Decode(
            "expected an unsigned amount of at most 256 bits".into(),
        ));
    }
    let amount =
        BigUint::parse_bytes(value.as_bytes(), 10).ok_or_else(|| invalid("invalid amount"))?;
    if amount.bits() > 256 {
        return Err(invalid("amount exceeds 256 bits"));
    }
    Ok(amount)
}
fn to_u64(value: &BigUint) -> Result<u64> {
    value
        .to_u64()
        .ok_or_else(|| invalid("satoshi amount or fee exceeds u64"))
}
fn sub(a: &BigUint, b: &BigUint) -> BigUint {
    if a > b { a - b } else { BigUint::ZERO }
}
fn ratio(n: BigUint, d: &BigUint) -> BigUint {
    if d.is_zero() { BigUint::ZERO } else { n / d }
}
fn scale(decimals: u32) -> Result<BigUint> {
    if decimals > 77 {
        return Err(invalid("unsupported token decimals"));
    }
    Ok(BigUint::from(10u32).pow(decimals))
}
fn rate(value: &str) -> Result<(BigUint, BigUint)> {
    let (whole, fractional) = value.split_once('.').unwrap_or((value, ""));
    let n = integer(&format!("{whole}{fractional}"))?;
    if n.is_zero() {
        return Err(invalid("peg rate must be positive"));
    }
    Ok((n, scale(fractional.len() as u32)?))
}

fn validate_dex(amount: &DexAmount, slippage: u16) -> Result<()> {
    let (DexAmount::ExactIn(value) | DexAmount::ExactOut(value)) = amount;
    if integer(value)?.is_zero() || slippage > 10_000 {
        return Err(invalid(
            "positive amount and slippage at most 10000 bps required",
        ));
    }
    Ok(())
}

fn is_zero_address(token: &str) -> bool {
    token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
        .is_some_and(|hex| hex.len() == 40 && hex.bytes().all(|b| b == b'0'))
}

/// Sources `/quote/lightning-send` prices: Arkade, an EVM hub, or native RBTC.
fn lightning_send_source(request: &QuoteRequest) -> Option<KnownChain> {
    let Chain::Known(chain) = &request.source_chain else {
        return None;
    };
    let native_rootstock =
        *chain == KnownChain::Rootstock && is_zero_address(request.source_token.as_wire_str());
    (native_rootstock
        || matches!(
            chain,
            KnownChain::Arkade | KnownChain::Ethereum | KnownChain::Polygon | KnownChain::Arbitrum
        ))
    .then(|| chain.clone())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LightningKind {
    Invoice,
    Address,
    Lnurl,
}

fn classify_lightning_destination(destination: &str) -> Option<LightningKind> {
    let destination = destination.trim().to_lowercase();
    if destination.contains('@') {
        Some(LightningKind::Address)
    } else if destination.starts_with("lnurl1") {
        Some(LightningKind::Lnurl)
    } else if destination.starts_with("ln") {
        Some(LightningKind::Invoice)
    } else {
        None
    }
}

fn can_compose(request: &QuoteRequest) -> bool {
    if request.bridge_source_chain.is_some() || request.bridge_target_chain.is_some() {
        return false;
    }
    let btc_source = request.source_token == TokenId::Btc;
    if btc_source == (request.target_token == TokenId::Btc) {
        return false;
    }
    if btc_source
        && request
            .target_chain
            .as_wire_str()
            .eq_ignore_ascii_case("solana")
    {
        return true;
    }
    let remote_token = if btc_source {
        &request.target_token
    } else {
        &request.source_token
    };
    remote_token.as_wire_str().starts_with("0x")
}

fn bridge_chain_name(chain: &str) -> Option<&'static str> {
    BRIDGE_CHAIN_NAMES
        .iter()
        .find(|(id, _)| *id == chain)
        .map(|(_, name)| *name)
}

/// The daemon serves only hub chains, so a bridge-only chain is quoted on its
/// Arbitrum counterpart with the real chain in the bridge field.
fn remap_bridge_chains(request: &mut QuoteRequest) -> Result<()> {
    let target = request.target_chain.as_wire_str();
    let bridge_target = if target.eq_ignore_ascii_case("solana") {
        Some(target.to_owned())
    } else {
        bridge_chain_name(target).map(str::to_owned)
    };
    // Rootstock is also a USDT0 destination, but RBTC settles on Rootstock itself.
    let native_rootstock = request.target_chain == Chain::rootstock()
        && is_zero_address(request.target_token.as_wire_str());
    if let Some(name) = bridge_target
        && !native_rootstock
    {
        let token = request.target_token.as_wire_str();
        request.target_token = if USDT0_ADDRESSES
            .iter()
            .any(|a| a.eq_ignore_ascii_case(token))
        {
            TokenId::Usdt0Arbitrum
        } else {
            TokenId::UsdcArbitrum
        };
        request.target_chain = Chain::arbitrum();
        request.bridge_target_chain = Some(name);
    }
    let source = request.source_chain.as_wire_str();
    if let Some(name) = bridge_chain_name(source)
        && let Some((_, usdc)) = CCTP_ONLY_USDC.iter().find(|(id, _)| *id == source)
    {
        if !request
            .source_token
            .as_wire_str()
            .eq_ignore_ascii_case(usdc)
        {
            return Err(invalid(&format!(
                "quote on {name} requires native USDC ({usdc}); got {}. Only USDC is bridgeable via CCTP",
                request.source_token.as_wire_str()
            )));
        }
        request.source_chain = Chain::arbitrum();
        request.source_token = TokenId::UsdcArbitrum;
        request.bridge_source_chain = Some(name.to_owned());
    }
    Ok(())
}

impl Client {
    /// Quote a swap the way the TypeScript SDK's `getQuote` does. A Lightning
    /// payout to [`QuoteRequest::lightning_destination`] from Arkade, an EVM hub
    /// (Ethereum, Polygon, Arbitrum) or native RBTC is priced by
    /// `/quote/lightning-send`. A BTC to EVM route in either direction, or BTC to
    /// Solana, is composed locally with [`DEFAULT_COMPOSE_SLIPPAGE_BPS`]. Anything
    /// else, or a failure of either step, is served by [`Self::get_server_quote`],
    /// whose error is returned.
    pub async fn get_quote(&self, request: QuoteRequest) -> Result<QuoteResponse> {
        if let Some(destination) = request.lightning_destination.as_deref()
            && request.target_chain == Chain::lightning()
            && let Some(source_chain) = lightning_send_source(&request)
        {
            match self
                .lightning_send_quote_as_quote(&request, source_chain, destination)
                .await
            {
                Ok(quote) => return Ok(quote),
                Err(error) => sdk_log!(
                    self.log,
                    Warn,
                    "client.getQuote.lightningSendFallback",
                    "exact lightning send quote failed; falling back to estimate",
                    "error" => error.to_string()
                ),
            }
        }
        if can_compose(&request) {
            match self
                .compose_quote(request.clone(), DEFAULT_COMPOSE_SLIPPAGE_BPS)
                .await
            {
                Ok(quote) => return Ok(quote),
                Err(Error::UnsupportedQuotePath(_)) => {}
                Err(error) => sdk_log!(
                    self.log,
                    Warn,
                    "client.getQuote.composeFallback",
                    "composeQuote failed; falling back to server /quote",
                    "error" => error.to_string()
                ),
            }
        }
        self.get_server_quote(request).await
    }

    /// `GET /quote` alone, with the client's referral code unless the request has
    /// one. When neither bridge field is set, a bridge-only chain is quoted on
    /// Arbitrum with the real chain in `bridge_target_chain` or
    /// `bridge_source_chain`, as the TypeScript SDK does; a CCTP-only source must be
    /// that chain's native USDC. A request that sets either bridge field is sent as
    /// given.
    pub async fn get_server_quote(&self, mut request: QuoteRequest) -> Result<QuoteResponse> {
        if request.referral_code.is_none() {
            request.referral_code = self.referral_code.clone();
        }
        if request.bridge_source_chain.is_none() && request.bridge_target_chain.is_none() {
            remap_bridge_chains(&mut request)?;
        }
        self.send(request).await
    }

    async fn lightning_send_quote_as_quote(
        &self,
        request: &QuoteRequest,
        source_chain: KnownChain,
        destination: &str,
    ) -> Result<QuoteResponse> {
        let kind = classify_lightning_destination(destination).ok_or_else(|| {
            invalid(&format!(
                "unrecognized lightning destination: {destination}"
            ))
        })?;
        let evm_source = source_chain != KnownChain::Arkade;
        // An invoice pins the payout itself; only addresses and LNURLs take the amount.
        let source_pinned =
            kind != LightningKind::Invoice && matches!(request.amount, QuoteAmount::Source(_));
        let sats =
            |amount: u128| u64::try_from(amount).map_err(|_| invalid("satoshi amount exceeds u64"));
        let raw = destination.to_owned();
        let destination = match (kind, &request.amount) {
            (LightningKind::Invoice, _) => LightningDestination::Invoice(raw),
            (LightningKind::Address, QuoteAmount::Source(amount)) => {
                LightningDestination::AddressSendMax {
                    address: raw,
                    amount: *amount,
                }
            }
            (LightningKind::Address, QuoteAmount::Target(amount)) => {
                LightningDestination::Address {
                    address: raw,
                    sats: sats(*amount)?,
                }
            }
            (LightningKind::Lnurl, QuoteAmount::Source(amount)) => {
                LightningDestination::LnurlSendMax {
                    lnurl: raw,
                    amount: *amount,
                }
            }
            (LightningKind::Lnurl, QuoteAmount::Target(amount)) => LightningDestination::Lnurl {
                lnurl: raw,
                sats: sats(*amount)?,
            },
        };
        let quote = self
            .lightning_send_quote_with_referral(
                source_chain,
                request.source_token.clone(),
                destination,
                request
                    .referral_code
                    .as_deref()
                    .or(self.referral_code.as_deref()),
                request.extra_fees_bps,
            )
            .await?;
        let target_sats = quote.target_amount_sats.to_string();
        // Arkade is 1:1, so a pinned amount is the pre-fee amount on both sides. An
        // EVM source reports the token amount that creation persists.
        let source_amount = if evm_source || source_pinned {
            quote.source_amount.clone()
        } else {
            target_sats.clone()
        };
        let target_amount = if !evm_source && source_pinned {
            quote.source_amount.clone()
        } else {
            target_sats.clone()
        };
        Ok(QuoteResponse {
            exchange_rate: quote.exchange_rate,
            network_fee: quote.network_fee_sats,
            gasless_network_fee: 0,
            protocol_fee: quote.protocol_fee_sats,
            protocol_fee_rate: quote.protocol_fee_rate,
            min_amount: quote.min_amount_sats,
            max_amount: quote.max_amount_sats,
            source_amount,
            target_amount,
            net_source_amount: quote.source_amount,
            net_target_amount: target_sats,
            bridge_fee: None,
        })
    }

    pub async fn get_dex_quote(&self, request: DexQuoteRequest) -> Result<DexQuoteResponse> {
        validate_dex(&request.amount, request.slippage_bps)?;
        let response = self
            .api_request(Method::POST, self.url("dex-quote")?)
            .json(&request)
            .send()
            .await?;
        Ok(check_status(response).await?.json().await?)
    }

    /// Fetch unsigned calls whose output lands on `sender`. Not for coordinator claims or
    /// funding, which need `get_dex_calldata_fund`. Callers must validate destinations and
    /// approvals before signing.
    pub async fn get_dex_calldata(
        &self,
        request: DexCalldataRequest,
    ) -> Result<DexCalldataResponse> {
        validate_dex(&request.amount, request.slippage_bps)?;
        let response = self
            .api_request(Method::POST, self.url("dex-calldata/claim")?)
            .json(&request)
            .send()
            .await?;
        Ok(check_status(response).await?.json().await?)
    }

    /// Funding/claim calls use the coordinator as recipient so its signed sweep
    /// enforces the user's destination and minimum output after DEX execution.
    pub async fn get_dex_calldata_fund(
        &self,
        request: DexCalldataRequest,
    ) -> Result<DexCalldataResponse> {
        validate_dex(&request.amount, request.slippage_bps)?;
        let response = self
            .api_request(Method::POST, self.url("dex-calldata/fund")?)
            .json(&request)
            .send()
            .await?;
        Ok(check_status(response).await?.json().await?)
    }

    /// Drop cached session-static quote primitives. Network and DEX fees are always fetched fresh.
    pub async fn clear_quote_cache(&self) {
        self.quote_cache.lock().await.0.clear();
    }

    async fn quote_static<T: Serialize + DeserializeOwned>(
        &self,
        key: String,
        fetch: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        // Holding the async mutex shares a single fetch among concurrent callers.
        // Cancellation or errors drop the guard without caching a failed request.
        let mut cache = self.quote_cache.lock().await;
        cache.0.retain(|_, (expiry, _)| *expiry > Instant::now());
        if let Some((_, value)) = cache.0.get(&key) {
            return serde_json::from_value(value.clone()).map_err(|e| Error::Decode(e.to_string()));
        }
        let value = fetch.await?;
        if cache.0.len() >= 128 {
            cache.0.clear();
        }
        cache.0.insert(
            key,
            (
                Instant::now() + Duration::from_secs(60),
                serde_json::to_value(&value).map_err(|e| Error::Decode(e.to_string()))?,
            ),
        );
        Ok(value)
    }

    /// Compose the same BTC↔EVM/Solana quote as the TypeScript SDK.
    /// Routes it cannot compose (BTC↔BTC, EVM↔EVM, Solana sources, explicit bridge
    /// flags, missing pairs) return [`Error::UnsupportedQuotePath`], and
    /// [`Self::get_quote`] sends those to the server.
    /// Chain/token discovery and referral rates cache for 60 seconds; fees and DEX pricing do not.
    pub async fn compose_quote(
        &self,
        request: QuoteRequest,
        slippage_bps: u16,
    ) -> Result<QuoteResponse> {
        let referral = request
            .referral_code
            .as_deref()
            .or(self.referral_code.as_deref());
        let rates = if referral.is_some() || request.extra_fees_bps.is_some() {
            ReferralRates::from(
                &self
                    .quote_static(
                        format!("referral:{:?}:{:?}", referral, request.extra_fees_bps),
                        self.get_referral_fee(referral, request.extra_fees_bps),
                    )
                    .await?,
            )
        } else {
            ReferralRates::default()
        };
        let pairs = self
            .quote_static("pairs".into(), self.get_swap_pairs())
            .await?;
        let config = self
            .quote_static("config".into(), self.get_chain_config())
            .await?;
        let fees = self.get_network_fees().await?;
        self.compose_quote_with_data(request, slippage_bps, &pairs, &fees, &config, rates)
            .await
    }

    /// Compose with caller-supplied discovery data; only the DEX leg may make an HTTP request.
    /// `referral` is the `/referral-fee` response, or a bare extra-fee rate (`f64`) when the
    /// pair fee applies.
    #[allow(clippy::too_many_arguments)]
    pub async fn compose_quote_with_data(
        &self,
        request: QuoteRequest,
        slippage_bps: u16,
        pairs: &SwapPairsResponse,
        fees: &NetworkFeesResponse,
        config: &ChainConfigResponse,
        referral: impl Into<ReferralRates>,
    ) -> Result<QuoteResponse> {
        let referral: ReferralRates = referral.into();
        let btc_source = request.source_token == TokenId::Btc;
        if btc_source == (request.target_token == TokenId::Btc) {
            return Err(unsupported(
                "quote composition requires exactly one BTC side",
            ));
        }
        if request.bridge_source_chain.is_some() || request.bridge_target_chain.is_some() {
            return Err(unsupported(
                "compose quotes with the actual remote chain/token; explicit bridge flags use get_server_quote",
            ));
        }
        if slippage_bps > 10_000 {
            return Err(invalid("slippage exceeds 10000 bps"));
        }
        let (remote_chain, remote_token, btc_chain) = if btc_source {
            (
                &request.target_chain,
                &request.target_token,
                &request.source_chain,
            )
        } else {
            (
                &request.source_chain,
                &request.source_token,
                &request.target_chain,
            )
        };
        if !matches!(
            btc_chain,
            Chain::Known(KnownChain::Bitcoin | KnownChain::Arkade | KnownChain::Lightning)
        ) {
            return Err(unsupported("BTC requires an explicit Bitcoin rail"));
        }
        let source_pinned = matches!(request.amount, QuoteAmount::Source(_));
        let (QuoteAmount::Source(pinned) | QuoteAmount::Target(pinned)) = request.amount;
        if pinned == 0 {
            return Err(invalid("amount must be positive"));
        }
        let pinned = BigUint::from(pinned);
        let direct = config.chains.iter().find(|c| &c.chain == remote_chain);
        let pivot = direct
            .or_else(|| config.chains.iter().find(|c| c.chain == Chain::arbitrum()))
            .ok_or_else(|| unsupported("settlement pivot missing from chain config"))?;
        let pivot_chain_id: u64 = pivot
            .chain
            .as_wire_str()
            .parse()
            .map_err(|_| unsupported("pivot chain must be numeric"))?;
        if !(8..=18).contains(&pivot.btc_pegged_token.decimals) {
            return Err(invalid("pivot decimals must be between 8 and 18"));
        }
        let pivot_scale = scale(pivot.btc_pegged_token.decimals - 8)?;
        let remote = if remote_chain.as_wire_str() == "Solana" && btc_source {
            DexToken::Solana {
                address: remote_token.as_wire_str().to_owned(),
            }
        } else {
            DexToken::Evm {
                chain_id: remote_chain
                    .as_wire_str()
                    .parse()
                    .map_err(|_| unsupported("unsupported remote chain"))?,
                address: remote_token.as_wire_str().to_ascii_lowercase(),
            }
        };
        let (pair_source, pair_target) = if btc_source {
            (btc_chain, &pivot.chain)
        } else {
            (&pivot.chain, btc_chain)
        };
        let pair = pairs
            .pairs
            .iter()
            .find(|p| &p.source == pair_source && &p.target == pair_target)
            .ok_or_else(|| unsupported("swap pair unavailable"))?;
        let fee = fees
            .pairs
            .iter()
            .find(|p| &p.source == pair_source && &p.target == pair_target)
            .ok_or_else(|| unsupported("network fee unavailable"))?;
        let network_fee = pair
            .network_fee_sats
            .checked_add(fee.fees.source_sats)
            .and_then(|v| v.checked_add(fee.fees.target_sats))
            .ok_or_else(|| invalid("network fee overflow"))?;
        let pivot_token = DexToken::Evm {
            chain_id: pivot_chain_id,
            address: pivot.btc_pegged_token.address.clone(),
        };
        let direct_pivot = direct.is_some()
            && remote_token
                .as_wire_str()
                .eq_ignore_ascii_case(&pivot.btc_pegged_token.address);
        let btc_pinned = btc_source == source_pinned;
        let (btc, evm, decimals, bridge_fee, bridge_native_fee) = if direct_pivot {
            let (num, den) = rate(&pivot.btc_peg_rate)?;
            let (btc, evm) = if btc_pinned {
                let evm = ((&pinned * num + &den / 2u32) / den) * &pivot_scale;
                (pinned, evm)
            } else {
                let btc = &pinned * den / (&pivot_scale * num);
                (btc, pinned)
            };
            (btc, evm, pivot.btc_pegged_token.decimals, BigUint::ZERO, 0)
        } else {
            let value = if btc_pinned {
                &pinned * &pivot_scale
            } else {
                pinned.clone()
            };
            let (from, to) = if btc_source {
                (pivot_token, remote)
            } else {
                (remote, pivot_token)
            };
            let dex = self
                .get_dex_quote(DexQuoteRequest {
                    from,
                    to,
                    amount: if source_pinned {
                        DexAmount::ExactIn(value.to_string())
                    } else {
                        DexAmount::ExactOut(value.to_string())
                    },
                    slippage_bps,
                    cctp_recipient_setup: request.bridge_recipient_setup,
                })
                .await?;
            let (btc, evm, decimals) = match (btc_source, source_pinned) {
                (true, true) => (
                    pinned,
                    integer(&dex.estimated_amount_out.raw)?,
                    dex.estimated_amount_out.decimals,
                ),
                (true, false) => (
                    integer(&dex.expected_amount_in.raw)? / &pivot_scale,
                    pinned,
                    dex.estimated_amount_out.decimals,
                ),
                (false, true) => (
                    integer(&dex.estimated_amount_out.raw)? / &pivot_scale,
                    pinned,
                    dex.expected_amount_in.decimals,
                ),
                (false, false) => (
                    pinned,
                    integer(&dex.expected_amount_in.raw)?,
                    dex.expected_amount_in.decimals,
                ),
            };
            let bridge_native_fee = match &dex.bridge_rate {
                Some(BridgeRate::Layerzero { native_fee_sats }) => {
                    to_u64(&integer(native_fee_sats)?)?
                }
                Some(BridgeRate::Cctp { .. }) | None => 0,
            };
            (
                btc,
                evm,
                decimals,
                integer(dex.bridge_fee.as_deref().unwrap_or("0"))?,
                bridge_native_fee,
            )
        };
        let network_fee = network_fee
            .checked_add(bridge_native_fee)
            .ok_or_else(|| invalid("network fee overflow"))?;
        // A negotiated partner rate replaces the pair fee before the extra fee is added.
        let effective_rate =
            referral.base_fee_rate.unwrap_or(pair.fee_percentage) + referral.extra_fee_rate;
        if !effective_rate.is_finite() || !(0.0..=1.0).contains(&effective_rate) {
            return Err(invalid("invalid protocol fee rate"));
        }
        let fee_scale = BigUint::from(10u32).pow(18);
        // Rates arrive as JSON numbers; match TypeScript's rounding of the rate alone.
        // Amount multiplication and all subsequent divisions remain integer-exact.
        let protocol_fee =
            to_u64(&(&btc * BigUint::from((effective_rate * 1e18).round() as u128) / fee_scale))?;
        let total = BigUint::from(network_fee) + BigUint::from(protocol_fee);
        let (source, target, net_source, net_target) = match (btc_source, source_pinned) {
            (true, true) => {
                let gross = &evm + &bridge_fee;
                let net = sub(&ratio(sub(&btc, &total) * &gross, &btc), &bridge_fee);
                (btc.clone(), gross, btc.clone(), net)
            }
            (true, false) => (btc.clone(), evm.clone(), &btc + &total, evm.clone()),
            (false, true) => (evm.clone(), btc.clone(), evm.clone(), sub(&btc, &total)),
            (false, false) => (
                evm.clone(),
                btc.clone(),
                ratio((&btc + &total) * &evm, &btc),
                btc.clone(),
            ),
        };
        let precision = scale(12)?;
        let exchange = ratio(&evm * scale(8)? * &precision, &(&btc * scale(decimals)?));
        let whole = &exchange / &precision;
        let fraction = format!("{:0>12}", (&exchange % &precision).to_string());
        let fraction = fraction.trim_end_matches('0');
        let exchange_rate = if fraction.is_empty() {
            whole.to_string()
        } else {
            format!("{whole}.{fraction}")
        };
        Ok(QuoteResponse {
            exchange_rate,
            network_fee,
            gasless_network_fee: 0,
            protocol_fee,
            protocol_fee_rate: effective_rate,
            min_amount: pair.min_sats,
            max_amount: pair.max_sats,
            source_amount: source.to_string(),
            target_amount: target.to_string(),
            net_source_amount: net_source.to_string(),
            net_target_amount: net_target.to_string(),
            bridge_fee: if bridge_fee.is_zero() {
                None
            } else {
                Some(to_u64(&bridge_fee)?)
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn expired_and_failed_static_values_are_fetched_again() {
        let client = Client::new("http://localhost:1").unwrap();
        client.quote_cache.lock().await.0.insert(
            "test".into(),
            (
                Instant::now() - Duration::from_secs(1),
                serde_json::json!(1),
            ),
        );
        assert_eq!(
            client
                .quote_static("test".into(), async { Ok(2u32) })
                .await
                .unwrap(),
            2
        );
        assert!(
            client
                .quote_static::<u32>("failure".into(), async {
                    Err(invalid("temporary failure"))
                })
                .await
                .is_err()
        );
        assert_eq!(
            client
                .quote_static("failure".into(), async { Ok(3u32) })
                .await
                .unwrap(),
            3
        );
        client.clear_quote_cache().await;
        assert_eq!(
            client
                .quote_static("test".into(), async { Ok(4u32) })
                .await
                .unwrap(),
            4
        );
    }
}
