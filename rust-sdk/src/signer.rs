//! HD signer for Lendaswap key derivation.
//!
//! The SDK doesn't hold funds — it only derives the keys it needs to sign
//! swap messages and prove ownership to the backend. If we ever add an
//! actual wallet (UTXO tracking, balance, broadcasts) it'll be a separate
//! feature on top of this signer.
//!
//! ## Derivation rules
//!
//! Mirrors `client-sdk/core/src/hd_wallet.rs` (the canonical reference) and
//! the ts-pure-sdk `Signer`. Every derivation goes through a BIP-32 master
//! `Xpriv` produced either from a BIP-39 mnemonic (`from_mnemonic`) or
//! from a base58check-encoded extended private key (`from_xprv`).
//!
//! | What | Path | Notes |
//! | --- | --- | --- |
//! | Per-swap signing key | `m/SIGNING_PREFIX'/LSW_IDENTIFIER'/{index}'` | `SIGNING_PREFIX = 83696968` (BIP-85), `LSW_IDENTIFIER = 121923` |
//! | User-ID parent xpub  | `m/ID_PREFIX'/LSW_IDENTIFIER'/0'` | hardened; published Xpub used for server-side recovery |
//! | Per-swap user_id     | `<user-id-xpub>/ID_PREFIX/LSW_IDENTIFIER/{index}` | non-hardened from the xpub above; reported as compressed pubkey |
//! | Per-swap EVM key     | `m/SIGNING_PREFIX'/LSW_IDENTIFIER'/{index}'/60'` | Lendaswap subtree per swap with hardened Ethereum coin-type child; one EOA per swap so concurrent funding deposits are disambiguable by address |
//!
//! The EVM key path is **not BIP-44 compatible** — by design, so the
//! Lendaswap deposit addresses don't pollute a user's standard Ethereum
//! wallet derivation. Each swap gets its own EOA; the merchant can run
//! many parallel swaps without funding ambiguity. The Permit2 "approve
//! once, reuse" optimisation doesn't apply (it never really did — Permit2
//! sigs are per-transfer), so there's no downside.
//!
//! ## Hash-lock construction
//!
//! - `preimage = BIP340_tagged_hash("lendaswap/preimage", secret_key)` where the tagged hash is
//!   `SHA256(SHA256(tag) || SHA256(tag) || data)`.
//! - `hash_lock = SHA256(preimage)` — what we send on the wire (`0x…`).
//! - Bitcoin HTLCs additionally use `HASH160(preimage)` on the unlock path; we don't compute that
//!   here because EVM→Arkade only needs the SHA256 form.

use crate::error::Error;
use crate::error::Result;
use bitcoin::Network;
use bitcoin::bip32::DerivationPath;
use bitcoin::bip32::Xpriv;
use bitcoin::bip32::Xpub;
use bitcoin::secp256k1::Secp256k1;
use sha2::Digest;
use sha2::Sha256;
use std::str::FromStr;

/// BIP-85 prefix for signing-key derivation.
const SIGNING_PREFIX: u32 = 83_696_968;
/// Lendaswap identifier ("LSW" encoded).
const LSW_IDENTIFIER: u32 = 121_923;
/// Prefix for identity (user-ID) derivation.
const ID_PREFIX: u32 = 9_419;
/// BIP-44 coin type for Ethereum — used as the hardened terminal segment
/// of the per-swap EVM key path.
const EVM_COIN_TYPE: u32 = 60;
/// BIP-340 tag for preimage tagged-hash domain separation.
const PREIMAGE_TAG: &str = "lendaswap/preimage";

/// HD signer holding the secret material the SDK derives swap keys from.
///
/// Cheap to clone (the internal `Xpriv` is `Copy`-shaped — bitcoin's
/// implementation copies cheaply on `.clone()`).
#[derive(Clone, Debug)]
pub struct Signer {
    /// Master extended private key. Derived once at construction; all
    /// derivations branch off this.
    master: Xpriv,
    /// Original mnemonic phrase if the signer was built from one. `None`
    /// when constructed from a raw xprv — matching the ts-pure-sdk's
    /// `Signer.mnemonic` getter semantics.
    mnemonic: Option<String>,
}

impl Signer {
    /// Build from a BIP-39 mnemonic (12 / 15 / 18 / 21 / 24 words). The
    /// phrase is trimmed and lowercased before validation.
    pub fn from_mnemonic(phrase: impl Into<String>) -> Result<Self> {
        let normalised = phrase.into().trim().to_lowercase();
        let mnemonic = bip39::Mnemonic::from_str(&normalised)
            .map_err(|e| Error::InvalidSigner(format!("invalid mnemonic: {e}")))?;
        let seed = mnemonic.to_seed("");
        let master = Xpriv::new_master(Network::Bitcoin, &seed).map_err(|e| {
            Error::InvalidSigner(format!("failed to derive master xpriv from seed: {e}"))
        })?;
        Ok(Self {
            master,
            mnemonic: Some(normalised),
        })
    }

    /// Build from a base58check-encoded BIP-32 extended private key. The
    /// network byte (mainnet `xprv…` vs testnet `tprv…`) is ignored — only
    /// the key material is used.
    pub fn from_xprv(xprv: impl Into<String>) -> Result<Self> {
        let trimmed = xprv.into().trim().to_string();
        let master = Xpriv::from_str(&trimmed)
            .map_err(|e| Error::InvalidSigner(format!("invalid xprv: {e}")))?;
        Ok(Self {
            master,
            mnemonic: None,
        })
    }

    /// The mnemonic this signer was built from, if any. Returns `None`
    /// when constructed via [`Self::from_xprv`].
    pub fn mnemonic(&self) -> Option<&str> {
        self.mnemonic.as_deref()
    }

    /// Derive per-swap parameters at `key_index`. Returns the signing
    /// secret/pubkey, the preimage + its SHA256 hash (the `hash_lock`),
    /// and the `user_id` pubkey the backend expects.
    pub fn derive_swap_params(&self, key_index: u32) -> Result<SwapParams> {
        let secp = Secp256k1::new();

        // Signing key: m/SIGNING_PREFIX'/LSW_IDENTIFIER'/index'
        let signing_path = parse_path(&format!(
            "m/{SIGNING_PREFIX}'/{LSW_IDENTIFIER}'/{key_index}'"
        ))?;
        let signing_xpriv = self
            .master
            .derive_priv(&secp, &signing_path)
            .map_err(derivation_err)?;
        let secret_key = signing_xpriv.private_key;
        let secret_bytes = secret_key.secret_bytes();
        let public_key = secret_key.public_key(&secp);
        let public_key_bytes = public_key.serialize();

        // Preimage = BIP340 tagged hash; hash_lock = SHA256(preimage).
        let preimage = tagged_hash(PREIMAGE_TAG, &secret_bytes);
        let hash_lock: [u8; 32] = Sha256::digest(preimage).into();

        // User ID: derive_user_id_xpub at m/ID_PREFIX'/LSW_IDENTIFIER'/0',
        // then non-hardened child m/ID_PREFIX/LSW_IDENTIFIER/{index}.
        let user_id_xpub = self.derive_user_id_xpub_inner(&secp)?;
        let user_id_inner = parse_path(&format!("m/{ID_PREFIX}/{LSW_IDENTIFIER}/{key_index}"))?;
        let user_id_pub = user_id_xpub
            .derive_pub(&secp, &user_id_inner)
            .map_err(derivation_err)?;
        let user_id_bytes = user_id_pub.public_key.serialize();

        Ok(SwapParams {
            secret: secret_bytes,
            preimage,
            hash_lock,
            public_key: public_key_bytes,
            user_id: user_id_bytes,
            key_index,
        })
    }

    /// Derive a per-swap EVM key at the given index. Path:
    /// `m/SIGNING_PREFIX'/LSW_IDENTIFIER'/{key_index}'/60'`. The derived
    /// EOA is dedicated to one swap so concurrent merchant flows don't
    /// share a funding address.
    pub fn derive_evm_key(&self, key_index: u32) -> Result<EvmKey> {
        let secp = Secp256k1::new();
        let path = parse_path(&format!(
            "m/{SIGNING_PREFIX}'/{LSW_IDENTIFIER}'/{key_index}'/{EVM_COIN_TYPE}'"
        ))?;
        let derived = self
            .master
            .derive_priv(&secp, &path)
            .map_err(derivation_err)?;
        let secret_key = derived.private_key;
        let public_key = secret_key.public_key(&secp);
        // EVM address: keccak256(uncompressed_pubkey[1..])[12..], lowercase hex.
        let uncompressed = public_key.serialize_uncompressed();
        let mut keccak = tiny_keccak::Keccak::v256();
        let mut digest = [0u8; 32];
        use tiny_keccak::Hasher as _;
        keccak.update(&uncompressed[1..]);
        keccak.finalize(&mut digest);
        let address = format!("0x{}", hex::encode(&digest[12..]));
        Ok(EvmKey {
            secret_key: secret_key.secret_bytes(),
            address,
        })
    }

    /// Xpub used as the parent for non-hardened `user_id` derivations.
    /// Hardened so a leaked child key doesn't compromise the parent.
    /// Public so it can be shared with the server for recovery — the
    /// server enumerates `user_id`s by deriving children of this xpub.
    pub fn derive_user_id_xpub(&self) -> Result<Xpub> {
        let secp = Secp256k1::new();
        self.derive_user_id_xpub_inner(&secp)
    }

    fn derive_user_id_xpub_inner(&self, secp: &Secp256k1<bitcoin::secp256k1::All>) -> Result<Xpub> {
        let path = parse_path(&format!("m/{ID_PREFIX}'/{LSW_IDENTIFIER}'/0'"))?;
        let xpriv = self
            .master
            .derive_priv(secp, &path)
            .map_err(derivation_err)?;
        Ok(Xpub::from_priv(secp, &xpriv))
    }
}

fn parse_path(s: &str) -> Result<DerivationPath> {
    DerivationPath::from_str(s).map_err(|e| Error::InvalidSigner(format!("invalid path {s}: {e}")))
}

fn derivation_err(e: bitcoin::bip32::Error) -> Error {
    Error::InvalidSigner(format!("BIP-32 derivation failed: {e}"))
}

/// BIP340-style tagged hash: `SHA256(SHA256(tag) || SHA256(tag) || data)`.
fn tagged_hash(tag: &str, data: &[u8]) -> [u8; 32] {
    let tag_hash = Sha256::digest(tag.as_bytes());
    let mut hasher = Sha256::new();
    hasher.update(tag_hash);
    hasher.update(tag_hash);
    hasher.update(data);
    hasher.finalize().into()
}

/// Per-swap key material produced by [`Signer::derive_swap_params`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapParams {
    /// Raw secret-key bytes (32). Sensitive — persisted via
    /// [`crate::SwapStorage`] keyed by swap ID so the client can sign
    /// claim transactions later. Re-derivable from
    /// `(mnemonic_or_xprv, key_index)`.
    pub secret: [u8; 32],
    /// `BIP-340_tagged_hash("lendaswap/preimage", secret)` — the HTLC
    /// preimage. The EVM-side HTLC unlocks on `SHA256(preimage) ==
    /// hash_lock`; the Bitcoin-side HTLC uses `HASH160(preimage)`.
    pub preimage: [u8; 32],
    /// `SHA256(preimage)` — the `hash_lock` field on the wire
    /// (`0x…` hex when serialised).
    pub hash_lock: [u8; 32],
    /// Compressed secp256k1 pubkey (33 bytes) for the swap signing key.
    /// Reported on the wire as `receiver_pk`.
    pub public_key: [u8; 33],
    /// Compressed pubkey of the non-hardened user-ID sibling. The backend
    /// uses it for recovery / per-swap routing.
    pub user_id: [u8; 33],
    /// The index used during derivation.
    pub key_index: u32,
}

/// EVM key material produced by [`Signer::derive_evm_key`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvmKey {
    /// secp256k1 secret scalar (32 bytes).
    pub secret_key: [u8; 32],
    /// `0x`-prefixed lowercase hex address (42 chars total).
    pub address: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Public BIP-39 test vector — produces a well-known master Xpriv.
    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn invalid_mnemonic_rejected() {
        assert!(Signer::from_mnemonic("not a valid mnemonic").is_err());
        assert!(Signer::from_mnemonic("").is_err());
    }

    #[test]
    fn known_mnemonic_round_trips() {
        let s1 = Signer::from_mnemonic(TEST_MNEMONIC).unwrap();
        let s2 = Signer::from_mnemonic(TEST_MNEMONIC).unwrap();
        let p1 = s1.derive_swap_params(0).unwrap();
        let p2 = s2.derive_swap_params(0).unwrap();
        assert_eq!(p1, p2);
        assert_eq!(s1.mnemonic(), Some(TEST_MNEMONIC));
    }

    #[test]
    fn different_indices_yield_different_params() {
        let s = Signer::from_mnemonic(TEST_MNEMONIC).unwrap();
        let p0 = s.derive_swap_params(0).unwrap();
        let p1 = s.derive_swap_params(1).unwrap();
        assert_ne!(p0.secret, p1.secret);
        assert_ne!(p0.public_key, p1.public_key);
        assert_ne!(p0.hash_lock, p1.hash_lock);
        assert_ne!(p0.user_id, p1.user_id);
    }

    #[test]
    fn hash_lock_is_sha256_of_preimage() {
        let s = Signer::from_mnemonic(TEST_MNEMONIC).unwrap();
        let params = s.derive_swap_params(0).unwrap();
        let expected: [u8; 32] = Sha256::digest(params.preimage).into();
        assert_eq!(params.hash_lock, expected);
    }

    #[test]
    fn evm_address_is_42_char_hex() {
        let s = Signer::from_mnemonic(TEST_MNEMONIC).unwrap();
        let evm = s.derive_evm_key(0).unwrap();
        assert!(evm.address.starts_with("0x"));
        assert_eq!(evm.address.len(), 42);
        assert!(evm.address[2..].chars().all(|c| c.is_ascii_hexdigit()));
        // Lowercase invariant.
        assert_eq!(evm.address, evm.address.to_lowercase());
    }

    #[test]
    fn different_indices_yield_different_evm_addresses() {
        let s = Signer::from_mnemonic(TEST_MNEMONIC).unwrap();
        let a = s.derive_evm_key(0).unwrap();
        let b = s.derive_evm_key(1).unwrap();
        let c = s.derive_evm_key(2).unwrap();
        assert_ne!(a.address, b.address);
        assert_ne!(b.address, c.address);
        assert_ne!(a.secret_key, b.secret_key);
    }

    #[test]
    fn same_index_yields_same_evm_address() {
        let s1 = Signer::from_mnemonic(TEST_MNEMONIC).unwrap();
        let s2 = Signer::from_mnemonic(TEST_MNEMONIC).unwrap();
        assert_eq!(s1.derive_evm_key(7).unwrap(), s2.derive_evm_key(7).unwrap());
    }

    #[test]
    fn from_xprv_round_trips_against_mnemonic() {
        let signer_mnemonic = Signer::from_mnemonic(TEST_MNEMONIC).unwrap();
        // Re-serialise the mnemonic-derived master and feed it back via xprv.
        let xprv_str = signer_mnemonic.master.to_string();
        let signer_xprv = Signer::from_xprv(&xprv_str).unwrap();
        assert!(signer_xprv.mnemonic().is_none());
        assert_eq!(
            signer_mnemonic.derive_swap_params(0).unwrap(),
            signer_xprv.derive_swap_params(0).unwrap(),
        );
        assert_eq!(
            signer_mnemonic.derive_evm_key(0).unwrap(),
            signer_xprv.derive_evm_key(0).unwrap(),
        );
    }

    #[test]
    fn invalid_xprv_rejected() {
        assert!(Signer::from_xprv("not an xprv").is_err());
        assert!(Signer::from_xprv("").is_err());
    }
}
