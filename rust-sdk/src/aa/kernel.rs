//! ZeroDev Kernel V3.3 — the EIP-7702 delegation target.
//!
//! Two protocol details this module owns:
//!
//! 1. **`execute(execMode, executionCalldata)` batch encoding.** Kernel's `execMode` is a packed
//!    `bytes32` (callType / execType / mode selector / mode payload); for our batch we set
//!    `CALLTYPE_BATCH (0x01) || EXECTYPE_DEFAULT (0x00) || zeros`. `executionCalldata` is
//!    `abi.encode(Execution[])` where `Execution = (address target, uint256 value, bytes callData)`
//!    — the single-param wrapping form (leading 0x20 offset word). Source:
//!    `src/utils/ExecLib.sol::encodeBatch` in the Kernel `v3.3` tag.
//!
//! 2. **Two distinct signature wrappings.**
//!    - The **UserOp** signature is the raw 65-byte ECDSA over the EIP-191-personal_sign-wrapped
//!      `userOpHash` — *no* validator prefix. Validator selection lives in `userOp.nonce`, not the
//!      signature. (`src/core/ValidationManager.sol::_verify7702Signature`)
//!    - The **ERC-1271** signature (the one Permit2 checks via `isValidSignature` on the smart
//!      account) is `0x00` (sudo-mode validator selector for the 7702 root validator) `||` ECDSA
//!      over Kernel's `Kernel(bytes32 hash)` EIP-712 wrapper of the inner hash. Domain:
//!      `name="Kernel"`, `version="0.3.3"`, `chainId`, `verifyingContract` = the smart account
//!      itself. (`src/core/ValidationManager.sol::_toWrappedHash` +
//!      `src/utils/ValidationTypeLib.sol::decodeSignature` sudo path)

use crate::aa::abi::Call;
use crate::aa::abi::IKernel;
use alloy::primitives::Address;
use alloy::primitives::B256;
use alloy::primitives::Bytes;
use alloy::primitives::address;
use alloy::primitives::b256;
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolStruct;
use alloy::sol_types::SolValue;
use alloy::sol_types::eip712_domain;

/// Kernel V3.3 implementation contract — the EIP-7702 delegation
/// target the depositor EOA gets upgraded to. Same address on every
/// EVM chain. Must move in lockstep with the backend's
/// `KERNEL_DELEGATION_TARGET_V3_3` (`swap_and_lock_userop_calldata.rs`).
pub const KERNEL_DELEGATION_TARGET: Address = address!("d6CEDDe84be40893d153Be9d467CD6aD37875b28");

/// EIP-712 domain `version` Kernel V3.3 uses in its `isValidSignature`
/// wrapper. Sourced from `src/Kernel.sol::_domainNameAndVersion()`.
pub const KERNEL_VERSION: &str = "0.3.3";

/// `execMode` for a CALLTYPE_BATCH / EXECTYPE_DEFAULT call:
/// `callType(1) || execType(1) || unused(4) || modeSelector(4) || modePayload(22)`
/// with the call-type set to BATCH (`0x01`) and everything else zero.
pub const EXEC_MODE_BATCH: B256 =
    b256!("0100000000000000000000000000000000000000000000000000000000000000");

/// Validator-selection prefix byte for the 7702 root-validator path in
/// `isValidSignature`. Hits the "sudo mode" branch of
/// `ValidationTypeLib::decodeSignature`, which strips 1 byte before
/// dispatching to `rootValidator`.
const ERC1271_SUDO_MODE_PREFIX: u8 = 0x00;

// Kernel's ERC-1271 hash-wrapping EIP-712 type.
//
// keccak256("Kernel(bytes32 hash)") =
// 0x1547321c374afde8a591d972a084b071c594c275e36724931ff96c25f2999c83
// (Kernel v3.3 src/types/Constants.sol::KERNEL_WRAPPER_TYPE_HASH,
// verified via `cast keccak` and pinned in `kernel_wrapper_typehash_matches_external_oracle`).
sol! {
    #[derive(Debug, PartialEq, Eq)]
    struct Kernel {
        bytes32 hash;
    }
}

/// Encode a Kernel `execute(execMode, executionCalldata)` call for a
/// batch of `calls`. Returns the full calldata including the 4-byte
/// function selector — drop straight into a `PackedUserOperation`'s
/// `callData`.
pub fn encode_execute_batch(calls: &[Call]) -> Bytes {
    // executionCalldata = abi.encode(Execution[]) — Solidity's single-
    // arg encoding. `(calls,).abi_encode_params()` matches that exactly:
    // a leading 0x20 offset to the array head, then length, then
    // per-element heads + tails.
    let execution_calldata = (calls,).abi_encode_params();
    let call = IKernel::executeCall {
        execMode: EXEC_MODE_BATCH,
        executionCalldata: execution_calldata.into(),
    };
    call.abi_encode().into()
}

/// Wrap a raw 65-byte ECDSA signature for use as the UserOp signature
/// field.
///
/// Identity wrap: Kernel V3.3 puts NO validator-selection prefix on the
/// UserOp signature (validator selection lives in `userOp.nonce`'s
/// `vType` byte). This helper exists to make the protocol decision
/// explicit and give it a single, documented home — call sites should
/// not paste raw signatures into the UserOp directly.
pub fn wrap_user_op_signature(signature: &[u8; 65]) -> Bytes {
    Bytes::copy_from_slice(signature)
}

/// Wrap a raw 65-byte ECDSA signature for Kernel V3.3's ERC-1271
/// `isValidSignature`.
///
/// Format: `0x00 || sig` — the leading byte is the "sudo mode" validator
/// selector, which routes verification to the 7702 root validator (i.e.
/// the delegated EOA's own key). Used for the Permit2 witness signature
/// the HTLC coordinator's Permit2 verifies via `isValidSignature`.
pub fn wrap_erc1271_signature(signature: &[u8; 65]) -> Bytes {
    let mut v = Vec::with_capacity(1 + 65);
    v.push(ERC1271_SUDO_MODE_PREFIX);
    v.extend_from_slice(signature);
    Bytes::from(v)
}

/// Produce the EIP-712 digest a 7702-delegated EOA must sign to satisfy
/// Kernel V3.3's `isValidSignature(inner_hash, …)`.
///
/// Wraps `inner_hash` (e.g. the Permit2 witness digest) in the Kernel
/// envelope `Kernel(bytes32 hash)`, using the smart-account address as
/// `verifyingContract`. The resulting digest is a *raw* EIP-712 hash —
/// the EOA signs it with plain ECDSA, NOT with the EIP-191
/// personal-sign prefix.
pub fn erc1271_wrapped_digest(inner_hash: B256, account: Address, chain_id: u64) -> B256 {
    let domain = eip712_domain! {
        name: "Kernel",
        version: KERNEL_VERSION,
        chain_id: chain_id,
        verifying_contract: account,
    };
    Kernel { hash: inner_hash }.eip712_signing_hash(&domain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;
    use alloy::primitives::address;

    /// Pinned, verified Kernel V3.3 `_domainNameAndVersion()` typehash.
    /// `cast keccak "Kernel(bytes32 hash)"`.
    #[test]
    fn kernel_wrapper_typehash_matches_external_oracle() {
        let kernel_type_hash =
            <Kernel as SolStruct>::eip712_type_hash(&Kernel { hash: B256::ZERO });
        assert_eq!(
            kernel_type_hash,
            b256!("1547321c374afde8a591d972a084b071c594c275e36724931ff96c25f2999c83"),
        );
    }

    /// CALLTYPE_BATCH = 0x01 in byte 0; the rest of the bytes32 is zero
    /// (EXECTYPE_DEFAULT + zero unused/selector/payload).
    #[test]
    fn exec_mode_batch_layout() {
        let mut expected = [0u8; 32];
        expected[0] = 0x01;
        assert_eq!(EXEC_MODE_BATCH, B256::from(expected));
    }

    /// `execute(bytes32,bytes)` selector, sourced from the Phase 0 spike
    /// (`cast sig`) — `0xe9ae5c53`. Cross-checks `sol!` against the
    /// authoritative selector.
    #[test]
    fn encode_execute_batch_selector() {
        let bytes = encode_execute_batch(&[]);
        assert_eq!(&bytes[..4], &[0xe9, 0xae, 0x5c, 0x53]);
    }

    /// Encoded batch decodes back to the same calls — proves the
    /// `executionCalldata` layout (single-param-wrapped `Execution[]`)
    /// matches Solidity's `abi.encode(Execution[])`.
    #[test]
    fn encode_execute_batch_round_trips() {
        let calls = vec![
            Call {
                target: address!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                value: U256::from(123u64),
                callData: Bytes::from_static(&[0x11, 0x22]),
            },
            Call {
                target: address!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
                value: U256::ZERO,
                callData: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
            },
        ];
        let encoded = encode_execute_batch(&calls);

        let decoded = IKernel::executeCall::abi_decode(&encoded).expect("call decodes");
        assert_eq!(decoded.execMode, EXEC_MODE_BATCH);

        let decoded_calls: Vec<Call> =
            <(Vec<Call>,)>::abi_decode_params(&decoded.executionCalldata)
                .expect("execution calldata decodes")
                .0;
        assert_eq!(decoded_calls, calls);
    }

    /// Empty batch is still well-formed: selector + an empty array.
    #[test]
    fn encode_execute_batch_handles_empty() {
        let encoded = encode_execute_batch(&[]);
        let decoded = IKernel::executeCall::abi_decode(&encoded).expect("call decodes");
        let decoded_calls: Vec<Call> =
            <(Vec<Call>,)>::abi_decode_params(&decoded.executionCalldata)
                .expect("execution calldata decodes")
                .0;
        assert!(decoded_calls.is_empty());
    }

    #[test]
    fn wrap_user_op_signature_is_identity() {
        let sig = [0xaa; 65];
        let wrapped = wrap_user_op_signature(&sig);
        assert_eq!(wrapped.len(), 65, "no prefix on UserOp sig");
        assert_eq!(&wrapped[..], &sig[..]);
    }

    #[test]
    fn wrap_erc1271_signature_prepends_sudo_byte() {
        let sig = [0xbb; 65];
        let wrapped = wrap_erc1271_signature(&sig);
        assert_eq!(wrapped.len(), 66, "1 prefix byte + 65 sig bytes");
        assert_eq!(wrapped[0], 0x00, "sudo-mode validator selector");
        assert_eq!(&wrapped[1..], &sig[..]);
    }

    #[test]
    fn erc1271_wrapped_digest_is_deterministic() {
        let inner = b256!("1234567890123456789012345678901234567890123456789012345678901234");
        let account = address!("1111111111111111111111111111111111111111");
        assert_eq!(
            erc1271_wrapped_digest(inner, account, 42161),
            erc1271_wrapped_digest(inner, account, 42161),
        );
    }

    #[test]
    fn erc1271_wrapped_digest_sensitive_to_each_input() {
        let inner = b256!("1234567890123456789012345678901234567890123456789012345678901234");
        let account = address!("1111111111111111111111111111111111111111");
        let base = erc1271_wrapped_digest(inner, account, 42161);

        // different inner hash
        let other_inner = b256!("abcdef1234567890123456789012345678901234567890123456789012345678");
        assert_ne!(erc1271_wrapped_digest(other_inner, account, 42161), base);

        // different account (= verifyingContract domain field)
        let other_account = address!("2222222222222222222222222222222222222222");
        assert_ne!(erc1271_wrapped_digest(inner, other_account, 42161), base);

        // different chainId
        assert_ne!(erc1271_wrapped_digest(inner, account, 1), base);
    }
}
