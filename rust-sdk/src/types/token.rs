//! Token identifiers.
//!
//! The spec defines `TokenId` as `oneOf [{ enum: ["btc"] }, { string }]` — so
//! the only "known" wire value is `"btc"`; EVM tokens are just contract
//! address strings. The [`well_known`] module below provides
//! discoverable constructors for the EVM tokens we expect callers to use
//! most often (USDC / USDT / WBTC on the named chains).

use super::chain::KnownChain;
use serde::Deserialize;
use serde::Serialize;

/// A token identifier. Either BTC or an EVM contract address.
///
/// Use [`TokenId::btc`] or [`TokenId::evm`] to construct, or one of the
/// helpers in [`well_known`] for common EVM tokens.
///
/// Wire format is a single string (`"btc"` or an EVM contract address); the
/// `from`/`into` serde attribute routes (de)serialization through `String`
/// because untagged enums with bare unit variants don't survive a string
/// round-trip.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(from = "String", into = "String")]
pub enum TokenId {
    Btc,
    Evm(String),
}

impl From<String> for TokenId {
    fn from(s: String) -> Self {
        if s == "btc" { Self::Btc } else { Self::Evm(s) }
    }
}

impl From<TokenId> for String {
    fn from(t: TokenId) -> Self {
        match t {
            TokenId::Btc => "btc".to_string(),
            TokenId::Evm(addr) => addr,
        }
    }
}

impl TokenId {
    pub fn btc() -> Self {
        Self::Btc
    }

    pub fn evm(address: impl Into<String>) -> Self {
        Self::Evm(address.into())
    }

    /// Wire representation as expected by the Lendaswap API.
    pub fn as_wire_str(&self) -> &str {
        match self {
            Self::Btc => "btc",
            Self::Evm(addr) => addr.as_str(),
        }
    }
}

/// Discoverable constructors for commonly-used EVM tokens.
///
/// Addresses are lowercase. The set is intentionally small — extend as
/// needed; for tokens we don't list here, callers can always use
/// [`TokenId::evm`] with the raw contract address.
pub mod well_known {
    use super::KnownChain;
    use super::TokenId;

    /// Native USDC on the given chain, when one is known.
    pub fn usdc(chain: KnownChain) -> Option<TokenId> {
        let addr = match chain {
            KnownChain::Polygon => "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359",
            KnownChain::Ethereum => "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            KnownChain::Arbitrum => "0xaf88d065e77c8cc2239327c5edb3a432268e5831",
            _ => return None,
        };
        Some(TokenId::Evm(addr.to_string()))
    }

    /// Tether USD (USDT) on the given chain, when one is known.
    pub fn usdt(chain: KnownChain) -> Option<TokenId> {
        let addr = match chain {
            KnownChain::Polygon => "0xc2132d05d31c914a87c6611c10748aeb04b58e8f",
            KnownChain::Ethereum => "0xdac17f958d2ee523a2206206994597c13d831ec7",
            KnownChain::Arbitrum => "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9",
            _ => return None,
        };
        Some(TokenId::Evm(addr.to_string()))
    }

    /// Wrapped Bitcoin (WBTC) on the given chain, when one is known.
    pub fn wbtc(chain: KnownChain) -> Option<TokenId> {
        let addr = match chain {
            KnownChain::Polygon => "0x1bfd67037b42cf73acf2047067bd4f2c47d9bfd6",
            KnownChain::Ethereum => "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599",
            KnownChain::Arbitrum => "0x2f2a2543b76a4166549f7aab2e75bef0aefc5b0f",
            _ => return None,
        };
        Some(TokenId::Evm(addr.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn btc_serialises_to_lowercase_string() {
        let v = serde_json::to_value(TokenId::btc()).unwrap();
        assert_eq!(v, serde_json::json!("btc"));
    }

    #[test]
    fn evm_address_round_trips() {
        let addr = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
        let parsed: TokenId = serde_json::from_value(serde_json::json!(addr)).unwrap();
        assert_eq!(parsed, TokenId::Evm(addr.to_string()));
        assert_eq!(parsed.as_wire_str(), addr);
    }

    #[test]
    fn deserialising_btc_string_prefers_btc_variant() {
        let parsed: TokenId = serde_json::from_value(serde_json::json!("btc")).unwrap();
        assert_eq!(parsed, TokenId::Btc);
    }

    #[test]
    fn well_known_usdc_returns_addresses_for_evm_chains() {
        assert!(well_known::usdc(KnownChain::Polygon).is_some());
        assert!(well_known::usdc(KnownChain::Arbitrum).is_some());
        assert!(well_known::usdc(KnownChain::Bitcoin).is_none());
    }
}
