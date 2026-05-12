//! ECDSA signing for the three gasless funding signatures.
//!
//! All three sign with the depositor's per-swap secp256k1 key (the
//! 32-byte secret from [`crate::signer::EvmKey`]) but differ in *what*
//! they sign:
//!
//! | Use                          | Wrapping                              | API                     |
//! |------------------------------|---------------------------------------|-------------------------|
//! | EIP-7702 authorization tuple | none — sign the raw 32-byte digest    | [`sign_hash`]           |
//! | Permit2 witness (wrapped)    | none — sign the Kernel-wrapped digest | [`sign_hash`]           |
//! | UserOp signature             | EIP-191 `personal_sign` over the hash | [`sign_eip191_message`] |
//!
//! Getting the EIP-191 vs raw distinction wrong silently fails at
//! verification — the Phase 0 spike flagged it as the worst footgun in
//! the stack, hence the two distinct entry points here.
//!
//! All signing is local (no network); `async` is purely API-shape
//! convention from `alloy::signers::Signer`.

use crate::error::Error;
use crate::error::Result;
use alloy::primitives::B256;
use alloy::primitives::Signature;
use alloy::signers::SignerSync;
use alloy::signers::local::PrivateKeySigner;

/// Sign a raw 32-byte digest with plain ECDSA — no EIP-191 prefix, no
/// EIP-712 wrapping. The caller has already done any
/// hashing/wrapping the protocol requires.
///
/// Used for:
/// - EIP-7702 authorization signatures (digest = `keccak256(0x05 || rlp([chainId, address,
///   nonce]))` from `alloy_eip7702::Authorization::signature_hash`).
/// - Permit2 witness signatures, *after* Kernel's `Kernel(bytes32 hash)` wrapping (digest from
///   [`crate::aa::kernel::erc1271_wrapped_digest`]).
pub fn sign_hash(secret_key: &[u8; 32], digest: B256) -> Result<Signature> {
    let signer = PrivateKeySigner::from_slice(secret_key)
        .map_err(|e| Error::InvalidSigner(format!("PrivateKeySigner::from_slice: {e}")))?;
    signer
        .sign_hash_sync(&digest)
        .map_err(|e| Error::InvalidSigner(format!("sign_hash: {e}")))
}

/// Sign `message` using EIP-191 `personal_sign` wrapping —
/// `keccak256("\x19Ethereum Signed Message:\n{len}" || message)` is
/// what actually gets signed.
///
/// For the UserOp signature, `message` is the 32-byte `userOpHash`
/// from [`crate::aa::userop::user_op_hash`]. Kernel V3.3's 7702 root
/// validator does `ECDSA.recover(toEthSignedMessageHash(userOpHash),
/// sig)` — i.e. it expects this exact prefix.
pub fn sign_eip191_message(secret_key: &[u8; 32], message: &[u8]) -> Result<Signature> {
    let signer = PrivateKeySigner::from_slice(secret_key)
        .map_err(|e| Error::InvalidSigner(format!("PrivateKeySigner::from_slice: {e}")))?;
    signer
        .sign_message_sync(message)
        .map_err(|e| Error::InvalidSigner(format!("sign_message: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    /// Test private key (Anvil account #0). The matching address is
    /// `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266` — same EOA every
    /// e2e test that uses Anvil's default mnemonic ends up with.
    const TEST_KEY: [u8; 32] = [
        0xac, 0x09, 0x74, 0xbe, 0xc3, 0x9a, 0x17, 0xe3, 0x6b, 0xa4, 0xa6, 0xb4, 0xd2, 0x38, 0xff,
        0x94, 0x4b, 0xac, 0xb4, 0x78, 0xcb, 0xed, 0x5e, 0xfc, 0xae, 0x78, 0x4d, 0x7b, 0xf4, 0xf2,
        0xff, 0x80,
    ];

    /// Sanity: the test key matches the known Anvil address.
    #[test]
    fn test_key_matches_anvil_address() {
        let signer = PrivateKeySigner::from_slice(&TEST_KEY).unwrap();
        assert_eq!(
            signer.address(),
            address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"),
        );
    }

    /// k256's RFC-6979 deterministic ECDSA: same key + same digest →
    /// identical signature. Pins the determinism guarantee since the
    /// orchestration depends on it for reproducible test failures.
    #[test]
    fn sign_hash_is_deterministic() {
        let digest = B256::repeat_byte(0x42);
        let a = sign_hash(&TEST_KEY, digest).unwrap();
        let b = sign_hash(&TEST_KEY, digest).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn sign_eip191_is_deterministic() {
        let message = b"hello, lendaswap";
        let a = sign_eip191_message(&TEST_KEY, message).unwrap();
        let b = sign_eip191_message(&TEST_KEY, message).unwrap();
        assert_eq!(a, b);
    }

    /// `sign_hash` (raw) and `sign_eip191_message` (prefix-wrapped)
    /// MUST produce different signatures for the same input bytes —
    /// they sign different digests. If they ever match for a given
    /// input, our two-entry-point story has collapsed.
    #[test]
    fn raw_vs_eip191_produce_different_signatures() {
        let digest = B256::repeat_byte(0xaa);
        let raw = sign_hash(&TEST_KEY, digest).unwrap();
        let eip191 = sign_eip191_message(&TEST_KEY, digest.as_slice()).unwrap();
        assert_ne!(
            raw, eip191,
            "raw-vs-EIP-191 must differ — getting this wrong breaks 7702/Permit2 verification",
        );
    }

    /// Recovering the signer address from the EIP-191 signature must
    /// reproduce the key's address — proves the signature is valid
    /// and `v` / `yParity` are encoded correctly.
    #[test]
    fn eip191_signature_recovers_to_signer_address() {
        let message = b"lendaswap roundtrip";
        let sig = sign_eip191_message(&TEST_KEY, message).unwrap();
        let recovered = sig
            .recover_address_from_msg(message)
            .expect("recover succeeds");
        assert_eq!(
            recovered,
            address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"),
        );
    }

    /// Same recovery check for raw-hash signing.
    #[test]
    fn raw_signature_recovers_to_signer_address() {
        let digest = B256::repeat_byte(0xbb);
        let sig = sign_hash(&TEST_KEY, digest).unwrap();
        let recovered = sig
            .recover_address_from_prehash(&digest)
            .expect("recover succeeds");
        assert_eq!(
            recovered,
            address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"),
        );
    }
}
