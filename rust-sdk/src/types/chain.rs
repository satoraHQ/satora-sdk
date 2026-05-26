//! Blockchain identifiers.
//!
//! The SDK exposes [`Chain`], which deserialises any wire string. Recognised
//! values land in [`Chain::Known`] with a typed [`KnownChain`] variant;
//! anything else round-trips through [`Chain::Other`]. This keeps the SDK
//! forward-compatible — when the backend starts returning a chain we haven't
//! named yet (e.g. `"Base"`), old clients still parse the response instead of
//! failing outright.

use serde::Deserialize;
use serde::Serialize;

/// Chains the SDK names explicitly. Marked `#[non_exhaustive]` so adding a new
/// variant in a future SDK release is not a breaking change for downstream
/// `match` expressions.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KnownChain {
    Arkade,
    Lightning,
    Bitcoin,
    #[serde(rename = "137")]
    Polygon,
    #[serde(rename = "1")]
    Ethereum,
    #[serde(rename = "42161")]
    Arbitrum,
}

impl KnownChain {
    /// Wire representation as expected by the Lendaswap API.
    pub fn as_wire_str(&self) -> &'static str {
        match self {
            Self::Arkade => "Arkade",
            Self::Lightning => "Lightning",
            Self::Bitcoin => "Bitcoin",
            Self::Polygon => "137",
            Self::Ethereum => "1",
            Self::Arbitrum => "42161",
        }
    }

    /// Parse a wire string into a known variant. Returns `None` if the value
    /// isn't recognised — callers should fall through to [`Chain::Other`].
    pub fn from_wire_str(s: &str) -> Option<Self> {
        match s {
            "Arkade" => Some(Self::Arkade),
            "Lightning" => Some(Self::Lightning),
            "Bitcoin" => Some(Self::Bitcoin),
            "137" => Some(Self::Polygon),
            "1" => Some(Self::Ethereum),
            "42161" => Some(Self::Arbitrum),
            _ => None,
        }
    }

    /// Numeric EVM chain ID. `None` for non-EVM rails (Bitcoin / Lightning /
    /// Arkade).
    pub fn evm_chain_id(&self) -> Option<u64> {
        match self {
            Self::Polygon => Some(137),
            Self::Ethereum => Some(1),
            Self::Arbitrum => Some(42161),
            Self::Bitcoin | Self::Lightning | Self::Arkade => None,
        }
    }

    /// Default public EVM node RPC URL for this chain. `None` for
    /// non-EVM rails (Bitcoin / Lightning / Arkade) and for any future
    /// EVM variant where we haven't picked a default yet.
    ///
    /// These are free / public endpoints — fine for low-volume use
    /// (BTCPay plugin checks, dev runs). Production deployments
    /// should override via `GaslessOpts.node_rpc_url` if they need
    /// higher throughput or guaranteed availability.
    pub fn default_node_rpc_url(&self) -> Option<&'static str> {
        match self {
            Self::Arbitrum => Some("https://arb1.arbitrum.io/rpc"),
            Self::Ethereum => Some("https://eth.llamarpc.com"),
            Self::Polygon => Some("https://polygon-rpc.com"),
            Self::Bitcoin | Self::Lightning | Self::Arkade => None,
        }
    }
}

/// A chain identifier. Either a [`KnownChain`] or an opaque wire string.
///
/// Construct from a wire string with [`Chain::from_wire_str`], which prefers
/// the known representation when possible (so `"137"` becomes
/// `Chain::Known(KnownChain::Polygon)`, not `Chain::Other("137")`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum Chain {
    Known(KnownChain),
    Other(String),
}

impl Chain {
    /// Parse any wire string, preferring the canonical [`Chain::Known`] form.
    pub fn from_wire_str(s: &str) -> Self {
        match KnownChain::from_wire_str(s) {
            Some(k) => Self::Known(k),
            None => Self::Other(s.to_string()),
        }
    }

    /// Wire representation. `Known` delegates to [`KnownChain::as_wire_str`];
    /// `Other` returns its raw string.
    pub fn as_wire_str(&self) -> &str {
        match self {
            Self::Known(k) => k.as_wire_str(),
            Self::Other(s) => s.as_str(),
        }
    }

    pub fn is_known(&self) -> bool {
        matches!(self, Self::Known(_))
    }

    /// Convenience constructors for the named chains.
    pub fn arkade() -> Self {
        Self::Known(KnownChain::Arkade)
    }
    pub fn lightning() -> Self {
        Self::Known(KnownChain::Lightning)
    }
    pub fn bitcoin() -> Self {
        Self::Known(KnownChain::Bitcoin)
    }
    pub fn polygon() -> Self {
        Self::Known(KnownChain::Polygon)
    }
    pub fn ethereum() -> Self {
        Self::Known(KnownChain::Ethereum)
    }
    pub fn arbitrum() -> Self {
        Self::Known(KnownChain::Arbitrum)
    }
}

impl From<KnownChain> for Chain {
    fn from(k: KnownChain) -> Self {
        Self::Known(k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_serialises_to_wire_string() {
        let v = serde_json::to_value(Chain::polygon()).unwrap();
        assert_eq!(v, serde_json::json!("137"));
    }

    #[test]
    fn other_round_trips() {
        let s = "Solana";
        let parsed: Chain = serde_json::from_value(serde_json::json!(s)).unwrap();
        assert_eq!(parsed, Chain::Other(s.to_string()));
        let back = serde_json::to_value(&parsed).unwrap();
        assert_eq!(back, serde_json::json!(s));
    }

    #[test]
    fn from_wire_str_canonicalises_known_values() {
        assert_eq!(Chain::from_wire_str("137"), Chain::polygon());
        assert_eq!(
            Chain::from_wire_str("Base"),
            Chain::Other("Base".to_string())
        );
    }

    #[test]
    fn deserialising_known_string_prefers_known_variant() {
        let parsed: Chain = serde_json::from_value(serde_json::json!("Arkade")).unwrap();
        assert!(parsed.is_known());
    }

    #[test]
    fn default_node_rpc_url_set_for_evm_chains() {
        assert_eq!(
            KnownChain::Arbitrum.default_node_rpc_url(),
            Some("https://arb1.arbitrum.io/rpc"),
        );
        assert_eq!(
            KnownChain::Ethereum.default_node_rpc_url(),
            Some("https://eth.llamarpc.com"),
        );
        assert_eq!(
            KnownChain::Polygon.default_node_rpc_url(),
            Some("https://polygon-rpc.com"),
        );
    }

    #[test]
    fn default_node_rpc_url_none_for_non_evm_rails() {
        assert_eq!(KnownChain::Bitcoin.default_node_rpc_url(), None);
        assert_eq!(KnownChain::Lightning.default_node_rpc_url(), None);
        assert_eq!(KnownChain::Arkade.default_node_rpc_url(), None);
    }
}
