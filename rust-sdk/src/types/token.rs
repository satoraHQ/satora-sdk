//! Token identifiers.
//!
//! [`TokenId`] is an enum of the tokens the SDK knows about by name, with
//! [`TokenId::Other`] as an escape hatch for contracts we haven't (yet)
//! named. The wire format is a single string — `"btc"` for Bitcoin, the
//! lowercase contract address for EVM tokens — so unrecognised wire values
//! deserialise into `Other` and round-trip cleanly until a release adds the
//! variant.
//!
//! The enum is `#[non_exhaustive]` so adding a new well-known variant in a
//! future SDK release is not a breaking change for downstream `match`es.

use super::chain::KnownChain;
use serde::Deserialize;
use serde::Serialize;

/// A token identifier.
///
/// Named variants serialise to their well-known wire string (`"btc"` for
/// Bitcoin, the lowercase contract address for EVM tokens). [`TokenId::Other`]
/// preserves any wire string the SDK doesn't recognise — see the module
/// docs for the forward-compatibility story.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(from = "String", into = "String")]
#[non_exhaustive]
pub enum TokenId {
    Btc,
    UsdcPolygon,
    UsdcArbitrum,
    UsdcEthereum,
    UsdtPolygon,
    UsdtEthereum,
    /// USDT0 — Tether's omnichain product on Arbitrum. Different contract
    /// from the legacy bridged USDT@Arbitrum (intentionally not modelled
    /// here; use [`TokenId::Other`] if you need it).
    Usdt0Arbitrum,
    WbtcPolygon,
    WbtcArbitrum,
    WbtcEthereum,
    /// Unrecognised wire value (a contract address or symbol the SDK doesn't
    /// know by name), preserved verbatim across the round trip.
    Other(String),
}

impl TokenId {
    /// Wire representation as expected by the Lendaswap API.
    pub fn as_wire_str(&self) -> &str {
        match self {
            Self::Btc => "btc",
            Self::UsdcPolygon => "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359",
            Self::UsdcArbitrum => "0xaf88d065e77c8cc2239327c5edb3a432268e5831",
            Self::UsdcEthereum => "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            Self::UsdtPolygon => "0xc2132d05d31c914a87c6611c10748aeb04b58e8f",
            Self::UsdtEthereum => "0xdac17f958d2ee523a2206206994597c13d831ec7",
            // USDT0 on Arbitrum reuses Tether's existing Arbitrum deployment as
            // the LayerZero OFT, so the address is the same one historically known
            // as "USDT@Arbitrum". Source: USDT0_ADDRESSES in
            // client-sdk/ts-pure-sdk/src/usdt0-bridge/constants.ts.
            Self::Usdt0Arbitrum => "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9",
            Self::WbtcPolygon => "0x1bfd67037b42cf73acf2047067bd4f2c47d9bfd6",
            Self::WbtcArbitrum => "0x2f2a2543b76a4166549f7aab2e75bef0aefc5b0f",
            Self::WbtcEthereum => "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599",
            Self::Other(s) => s.as_str(),
        }
    }

    /// Chain this token natively lives on, if known. Returns `None` for
    /// [`TokenId::Btc`] (rail-ambiguous: depends on whether the BTC sits on
    /// Bitcoin / Lightning / Arkade) and [`TokenId::Other`] (unrecognised).
    pub fn chain(&self) -> Option<KnownChain> {
        match self {
            Self::UsdcPolygon | Self::UsdtPolygon | Self::WbtcPolygon => Some(KnownChain::Polygon),
            Self::UsdcArbitrum | Self::Usdt0Arbitrum | Self::WbtcArbitrum => {
                Some(KnownChain::Arbitrum)
            }
            Self::UsdcEthereum | Self::UsdtEthereum | Self::WbtcEthereum => {
                Some(KnownChain::Ethereum)
            }
            Self::Btc | Self::Other(_) => None,
        }
    }

    /// USDC on the given chain, if the SDK has a named variant for it.
    pub fn usdc_on(chain: KnownChain) -> Option<Self> {
        match chain {
            KnownChain::Polygon => Some(Self::UsdcPolygon),
            KnownChain::Arbitrum => Some(Self::UsdcArbitrum),
            KnownChain::Ethereum => Some(Self::UsdcEthereum),
            _ => None,
        }
    }

    /// Tether on the given chain. Returns [`TokenId::Usdt0Arbitrum`] for
    /// Arbitrum — USDT0 is the omnichain Tether product we model there; the
    /// legacy bridged USDT@Arbitrum is not named.
    pub fn usdt_on(chain: KnownChain) -> Option<Self> {
        match chain {
            KnownChain::Polygon => Some(Self::UsdtPolygon),
            KnownChain::Ethereum => Some(Self::UsdtEthereum),
            KnownChain::Arbitrum => Some(Self::Usdt0Arbitrum),
            _ => None,
        }
    }

    /// WBTC on the given chain, if the SDK has a named variant for it.
    pub fn wbtc_on(chain: KnownChain) -> Option<Self> {
        match chain {
            KnownChain::Polygon => Some(Self::WbtcPolygon),
            KnownChain::Arbitrum => Some(Self::WbtcArbitrum),
            KnownChain::Ethereum => Some(Self::WbtcEthereum),
            _ => None,
        }
    }
}

impl From<String> for TokenId {
    fn from(s: String) -> Self {
        // Backend may return either lowercase or checksummed (mixed-case)
        // addresses; normalise before matching so both shapes survive the
        // round trip into a named variant.
        let lower = s.to_lowercase();
        match lower.as_str() {
            "btc" => Self::Btc,
            "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359" => Self::UsdcPolygon,
            "0xaf88d065e77c8cc2239327c5edb3a432268e5831" => Self::UsdcArbitrum,
            "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48" => Self::UsdcEthereum,
            "0xc2132d05d31c914a87c6611c10748aeb04b58e8f" => Self::UsdtPolygon,
            "0xdac17f958d2ee523a2206206994597c13d831ec7" => Self::UsdtEthereum,
            "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9" => Self::Usdt0Arbitrum,
            "0x1bfd67037b42cf73acf2047067bd4f2c47d9bfd6" => Self::WbtcPolygon,
            "0x2f2a2543b76a4166549f7aab2e75bef0aefc5b0f" => Self::WbtcArbitrum,
            "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599" => Self::WbtcEthereum,
            _ => Self::Other(s),
        }
    }
}

impl From<TokenId> for String {
    fn from(t: TokenId) -> Self {
        t.as_wire_str().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn btc_serialises_to_lowercase_string() {
        let v = serde_json::to_value(TokenId::Btc).unwrap();
        assert_eq!(v, serde_json::json!("btc"));
    }

    #[test]
    fn known_evm_variant_serialises_to_its_address() {
        let v = serde_json::to_value(TokenId::UsdcPolygon).unwrap();
        assert_eq!(
            v,
            serde_json::json!("0x3c499c542cef5e3811e1192ce70d8cc03d5c3359"),
        );
    }

    #[test]
    fn known_address_deserialises_to_named_variant() {
        let parsed: TokenId = serde_json::from_value(serde_json::json!(
            "0xaf88d065e77c8cc2239327c5edb3a432268e5831"
        ))
        .unwrap();
        assert_eq!(parsed, TokenId::UsdcArbitrum);
    }

    #[test]
    fn checksummed_address_deserialises_to_named_variant() {
        let parsed: TokenId = serde_json::from_value(serde_json::json!(
            "0xAF88D065E77C8CC2239327C5EDB3A432268E5831"
        ))
        .unwrap();
        assert_eq!(parsed, TokenId::UsdcArbitrum);
    }

    #[test]
    fn unknown_address_falls_through_to_other() {
        let addr = "0xdeadbeef0000000000000000000000000000beef";
        let parsed: TokenId = serde_json::from_value(serde_json::json!(addr)).unwrap();
        assert_eq!(parsed, TokenId::Other(addr.to_string()));
        assert_eq!(parsed.as_wire_str(), addr);
    }

    #[test]
    fn deserialising_btc_string_prefers_btc_variant() {
        let parsed: TokenId = serde_json::from_value(serde_json::json!("btc")).unwrap();
        assert_eq!(parsed, TokenId::Btc);
    }

    #[test]
    fn usdc_on_known_evm_chains_returns_variant() {
        assert_eq!(
            TokenId::usdc_on(KnownChain::Polygon),
            Some(TokenId::UsdcPolygon),
        );
        assert_eq!(
            TokenId::usdc_on(KnownChain::Arbitrum),
            Some(TokenId::UsdcArbitrum),
        );
        assert_eq!(TokenId::usdc_on(KnownChain::Bitcoin), None);
    }

    #[test]
    fn usdt_on_arbitrum_returns_usdt0() {
        assert_eq!(
            TokenId::usdt_on(KnownChain::Arbitrum),
            Some(TokenId::Usdt0Arbitrum),
        );
    }

    #[test]
    fn chain_recovers_token_chain() {
        assert_eq!(TokenId::UsdcPolygon.chain(), Some(KnownChain::Polygon));
        assert_eq!(TokenId::Usdt0Arbitrum.chain(), Some(KnownChain::Arbitrum));
        assert_eq!(TokenId::Btc.chain(), None);
        assert_eq!(TokenId::Other("xyz".into()).chain(), None);
    }
}
