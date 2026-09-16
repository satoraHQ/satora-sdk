use super::Client;
use super::check_status;
use crate::Error;
use crate::Result;
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

#[derive(Default)]
pub(super) struct QuoteCache(HashMap<String, (Instant, Value)>);

fn invalid(message: &str) -> Error {
    Error::InvalidSwap(message.into())
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

impl Client {
    pub async fn get_dex_quote(&self, request: DexQuoteRequest) -> Result<DexQuoteResponse> {
        validate_dex(&request.amount, request.slippage_bps)?;
        let response = self
            .api_request(Method::POST, self.url("dex-quote")?)
            .json(&request)
            .send()
            .await?;
        Ok(check_status(response).await?.json().await?)
    }

    /// Fetch unsigned calls. Callers must validate destinations and approvals before signing.
    pub async fn get_dex_calldata(
        &self,
        request: DexCalldataRequest,
    ) -> Result<DexCalldataResponse> {
        validate_dex(&request.amount, request.slippage_bps)?;
        let response = self
            .api_request(Method::POST, self.url("dex-calldata")?)
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
    /// BTC↔BTC, EVM↔EVM and Solana sources use [`Self::get_quote`] instead.
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
        let extra = if referral.is_some() || request.extra_fees_bps.is_some() {
            self.quote_static(
                format!("referral:{:?}:{:?}", referral, request.extra_fees_bps),
                self.get_referral_fee(referral, request.extra_fees_bps),
            )
            .await?
            .extra_fee_rate
        } else {
            0.0
        };
        let pairs = self
            .quote_static("pairs".into(), self.get_swap_pairs())
            .await?;
        let config = self
            .quote_static("config".into(), self.get_chain_config())
            .await?;
        let fees = self.get_network_fees().await?;
        self.compose_quote_with_data(request, slippage_bps, &pairs, &fees, &config, extra)
            .await
    }

    /// Compose with caller-supplied discovery data; only the DEX leg may make an HTTP request.
    #[allow(clippy::too_many_arguments)]
    pub async fn compose_quote_with_data(
        &self,
        request: QuoteRequest,
        slippage_bps: u16,
        pairs: &SwapPairsResponse,
        fees: &NetworkFeesResponse,
        config: &ChainConfigResponse,
        referral_rate: f64,
    ) -> Result<QuoteResponse> {
        let btc_source = request.source_token == TokenId::Btc;
        if btc_source == (request.target_token == TokenId::Btc) {
            return Err(invalid("quote composition requires exactly one BTC side"));
        }
        if request.bridge_source_chain.is_some() || request.bridge_target_chain.is_some() {
            return Err(invalid(
                "compose quotes with the actual remote chain/token; explicit bridge flags use get_quote",
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
            return Err(invalid("BTC requires an explicit Bitcoin rail"));
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
            .ok_or_else(|| invalid("settlement pivot missing from chain config"))?;
        let pivot_chain_id: u64 = pivot
            .chain
            .as_wire_str()
            .parse()
            .map_err(|_| invalid("pivot chain must be numeric"))?;
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
                    .map_err(|_| invalid("unsupported remote chain"))?,
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
            .ok_or_else(|| invalid("swap pair unavailable"))?;
        let fee = fees
            .pairs
            .iter()
            .find(|p| &p.source == pair_source && &p.target == pair_target)
            .ok_or_else(|| invalid("network fee unavailable"))?;
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
        let (btc, evm, decimals, bridge_fee) = if direct_pivot {
            let (num, den) = rate(&pivot.btc_peg_rate)?;
            let (btc, evm) = if btc_pinned {
                let evm = ((&pinned * num + &den / 2u32) / den) * &pivot_scale;
                (pinned, evm)
            } else {
                let btc = &pinned * den / (&pivot_scale * num);
                (btc, pinned)
            };
            (btc, evm, pivot.btc_pegged_token.decimals, BigUint::ZERO)
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
            (
                btc,
                evm,
                decimals,
                integer(dex.bridge_fee.as_deref().unwrap_or("0"))?,
            )
        };
        let effective_rate = pair.fee_percentage + referral_rate;
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
