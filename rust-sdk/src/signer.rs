//! HD signer for Lendaswap key derivation.
//!
//! The SDK doesn't hold funds — it only derives the keys it needs to sign
//! swap messages and prove ownership to the backend. If we ever add an
//! actual wallet (UTXO tracking, balance, broadcasts) it'll be a separate
//! feature on top of this signer.
//!
//! Phase 1 (this revision): wires up the construction surface — `Signer`
//! accepts either a BIP-39 mnemonic or a BIP-32 extended private key
//! (xprv) — but every derivation method is `todo!()`. The interface is
//! stable; Phase 2 fills in real BIP-32 / secp256k1 paths matching
//! `client-sdk/core/src/hd_wallet.rs` and the ts-pure-sdk `Signer`.
//!
//! Derivation rules we'll implement in Phase 2 (documented here so the
//! interface freeze is meaningful):
//!
//! - Per-swap signing key: `m/{SIGNING_PREFIX}'/{LSW_IDENTIFIER}'/{index}'` where `SIGNING_PREFIX =
//!   83696968` (BIP-85), `LSW_IDENTIFIER = 121923`.
//! - Per-swap user ID: non-hardened sibling derivation (so the xpub can be shared with the server
//!   for recovery).
//! - Preimage: BIP-340 tagged hash of the secret key with tag `"lendaswap/preimage"`. `hash_lock =
//!   SHA256(preimage)`.
//! - EVM key: `m/44'/60'/0'/0/0`. EVM address is the last 20 bytes of
//!   `keccak256(uncompressed_pubkey[1..])`, `0x`-prefixed lowercase.

use crate::error::Error;
use crate::error::Result;

/// HD signer holding the secret material the SDK derives swap keys from.
///
/// Cheap to clone (just clones a string). Real key derivation happens
/// lazily in Phase 2; today the derivation methods panic with `todo!()`.
#[derive(Clone, Debug)]
pub struct Signer {
    secret: Secret,
}

/// Internal representation of the signer's root secret. Kept private so
/// the outer struct can stay clone-cheap and so we can change
/// representation (e.g. parse to `bitcoin::bip32::Xpriv` up front) in
/// Phase 2 without a breaking change.
#[derive(Clone, Debug)]
#[allow(dead_code)] // Phase 2 reads these once derivation is real.
enum Secret {
    /// Validated BIP-39 mnemonic phrase, normalised to lowercase.
    Mnemonic(String),
    /// BIP-32 extended private key (base58check).
    Xprv(String),
}

impl Signer {
    /// Construct from a BIP-39 mnemonic (12 / 15 / 18 / 21 / 24 words).
    ///
    /// Phase 1 stores the phrase verbatim after trimming + lowercasing.
    /// Phase 2 will validate against the BIP-39 wordlist.
    pub fn from_mnemonic(phrase: impl Into<String>) -> Result<Self> {
        let normalised = phrase.into().trim().to_lowercase();
        if normalised.is_empty() {
            return Err(Error::InvalidSigner("mnemonic phrase is empty".to_string()));
        }
        Ok(Self {
            secret: Secret::Mnemonic(normalised),
        })
    }

    /// Construct from a BIP-32 extended private key (xprv) — base58check.
    ///
    /// Phase 1 stores the string after a trim. Phase 2 will validate via
    /// `bitcoin::bip32::Xpriv::from_str` (or equivalent) and reject xpubs.
    pub fn from_xprv(xprv: impl Into<String>) -> Result<Self> {
        let trimmed = xprv.into().trim().to_string();
        if trimmed.is_empty() {
            return Err(Error::InvalidSigner("xprv is empty".to_string()));
        }
        Ok(Self {
            secret: Secret::Xprv(trimmed),
        })
    }

    /// Derive per-swap parameters at `key_index`. Returns the secret,
    /// preimage, `hash_lock`, the swap signing pubkey, and the `user_id`
    /// the backend expects.
    ///
    /// **Phase 1: stubbed.** Calls panic via `todo!()` — Phase 2 will
    /// implement the BIP-32 derivation.
    pub fn derive_swap_params(&self, _key_index: u32) -> Result<SwapParams> {
        let _ = &self.secret;
        todo!("Phase 2: BIP-32 derivation at m/SIGNING_PREFIX'/LSW_IDENTIFIER'/index'")
    }

    /// Derive the user's EVM key (BIP44 path `m/44'/60'/0'/0/0`) and its
    /// `0x…` address.
    ///
    /// **Phase 1: stubbed.**
    pub fn derive_evm_key(&self) -> Result<EvmKey> {
        let _ = &self.secret;
        todo!("Phase 2: BIP-32 derivation at m/44'/60'/0'/0/0 + keccak256 address")
    }
}

/// Per-swap key material produced by [`Signer::derive_swap_params`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapParams {
    /// Random secret (32 bytes); never sent to the backend, persisted via
    /// [`crate::SwapStorage`] for later claim.
    pub secret: [u8; 32],
    /// `BIP-340_tagged_hash("lendaswap/preimage", secret)` — the HTLC
    /// preimage. The Bitcoin-side HTLC uses `HASH160(preimage)`; the EVM
    /// side uses `SHA256(preimage)` (the hash lock below).
    pub preimage: [u8; 32],
    /// `SHA256(preimage)` — the `hash_lock` field on the wire (`0x`-prefixed
    /// hex when serialised).
    pub hash_lock: [u8; 32],
    /// Compressed pubkey for the swap signing key (33 bytes). Reported on
    /// the wire as `receiver_pk` in EVM→Arkade and similar flows.
    pub public_key: [u8; 33],
    /// Compressed pubkey of the non-hardened user-ID sibling key (33
    /// bytes). The backend uses this for recovery.
    pub user_id: [u8; 33],
    /// The index used during derivation. Echo back to disambiguate when
    /// multiple swaps come back at once.
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
