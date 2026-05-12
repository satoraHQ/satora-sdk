//! Typed addresses (receive / refund destinations).
//!
//! [`Address`] carries the raw string plus the kind of address it is, so
//! higher-level SDK methods can check that the address kind matches the
//! target chain of a swap before sending a request. Phase 1 doesn't parse
//! the address contents — it's a tagged wrapper. Stronger validation
//! (bech32 / checksum / network match) can land here later without
//! touching call sites.

use super::chain::Chain;
use super::chain::KnownChain;

/// A destination address. The variant identifies which kind of chain the
/// address is for; the inner `String` is the raw wire form.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Address {
    /// Arkade VTXO address (bech32m, `ark1q…`).
    Arkade(String),
    /// On-chain Bitcoin address (any flavour).
    Bitcoin(String),
    /// Lightning BOLT11 invoice or LNURL.
    Lightning(String),
    /// EVM address (`0x…`, 20-byte hex).
    Evm(String),
}

impl Address {
    /// Raw wire string.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Arkade(s) | Self::Bitcoin(s) | Self::Lightning(s) | Self::Evm(s) => s.as_str(),
        }
    }

    /// Which chain this address kind expects. EVM is reported as `None`
    /// because the same EVM address can be used on any EVM chain.
    pub fn chain(&self) -> Option<KnownChain> {
        match self {
            Self::Arkade(_) => Some(KnownChain::Arkade),
            Self::Bitcoin(_) => Some(KnownChain::Bitcoin),
            Self::Lightning(_) => Some(KnownChain::Lightning),
            Self::Evm(_) => None,
        }
    }

    /// `true` when this address is valid as a destination for the given
    /// target chain. Arkade↔Arkade, Bitcoin↔Bitcoin, etc.; any
    /// [`Address::Evm`] satisfies any EVM chain because EVM addresses are
    /// not chain-specific.
    pub fn fits_chain(&self, target: &Chain) -> bool {
        let target_known = match target {
            Chain::Known(k) => k.clone(),
            Chain::Other(_) => return matches!(self, Self::Evm(_)),
        };
        match (self, target_known) {
            (Self::Arkade(_), KnownChain::Arkade)
            | (Self::Bitcoin(_), KnownChain::Bitcoin)
            | (Self::Lightning(_), KnownChain::Lightning) => true,
            (Self::Evm(_), kc) => kc.evm_chain_id().is_some(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arkade_address_fits_arkade_chain() {
        let a = Address::Arkade("ark1q...".to_string());
        assert!(a.fits_chain(&Chain::arkade()));
        assert!(!a.fits_chain(&Chain::bitcoin()));
    }

    #[test]
    fn evm_address_fits_any_evm_chain() {
        let a = Address::Evm("0xabc".to_string());
        assert!(a.fits_chain(&Chain::polygon()));
        assert!(a.fits_chain(&Chain::arbitrum()));
        assert!(a.fits_chain(&Chain::ethereum()));
        assert!(!a.fits_chain(&Chain::bitcoin()));
    }

    #[test]
    fn lightning_address_only_fits_lightning() {
        let a = Address::Lightning("lnbc1...".to_string());
        assert!(a.fits_chain(&Chain::lightning()));
        assert!(!a.fits_chain(&Chain::bitcoin()));
        assert!(!a.fits_chain(&Chain::arbitrum()));
    }
}
