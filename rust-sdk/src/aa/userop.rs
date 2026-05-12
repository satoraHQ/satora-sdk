//! ERC-4337 EntryPoint v0.7 `PackedUserOperation` helpers.
//!
//! This phase: the `userOpHash` computation. Full assembly of a
//! populated user operation (callData, gas fields, paymaster, signature)
//! lands with the kernel + bundler phases.
//!
//! Note on the type name: alloy's `PackedUserOperation` is, despite its
//! name, the *unpacked* v0.7 RPC representation — `factory` /
//! `factory_data` are separate, gas limits / fees are separate `U256`
//! fields. EntryPoint v0.7's on-chain `getUserOpHash`, however, hashes
//! the *packed* form (`initCode`, `accountGasLimits`, `gasFees`,
//! `paymasterAndData`). So [`user_op_hash`] does the packing itself.

use alloy::primitives::Address;
use alloy::primitives::B256;
use alloy::primitives::Bytes;
use alloy::primitives::U256;
use alloy::primitives::keccak256;
use alloy::rpc::types::erc4337::PackedUserOperation;
use alloy::sol_types::SolValue;

/// Compute the EntryPoint v0.7 `userOpHash` — the digest the account's
/// signature signs.
///
/// Mirrors the reference EntryPoint v0.7 implementation:
///
/// ```text
/// userOpHash = keccak256(abi.encode(
///     keccak256(abi.encode(
///         sender,
///         nonce,
///         keccak256(initCode),
///         keccak256(callData),
///         accountGasLimits,
///         preVerificationGas,
///         gasFees,
///         keccak256(paymasterAndData)
///     )),
///     entryPoint,
///     chainId
/// ))
/// ```
///
/// `signature` is deliberately excluded — it's what this hash is signed
/// *into*.
///
/// Solidity's `abi.encode(a, b, …)` is a parameter sequence (no outer
/// tuple offset) — that's `SolValue::abi_encode_params` on the tuple,
/// not `abi_encode`.
///
/// The structural tests below cover determinism + per-field
/// sensitivity. The authoritative cross-check against a real bundler's
/// view of the hash happens in the bundler phase, via a live
/// `eth_estimateUserOperationGas` round-trip.
pub fn user_op_hash(op: &PackedUserOperation, entry_point: Address, chain_id: u64) -> B256 {
    let init_code = pack_init_code(op);
    let account_gas_limits = pack_u128_pair(op.verification_gas_limit, op.call_gas_limit);
    let gas_fees = pack_u128_pair(op.max_priority_fee_per_gas, op.max_fee_per_gas);
    let paymaster_and_data = pack_paymaster_and_data(op);

    let inner = keccak256(
        (
            op.sender,
            op.nonce,
            keccak256(&init_code),
            keccak256(&op.call_data),
            account_gas_limits,
            op.pre_verification_gas,
            gas_fees,
            keccak256(&paymaster_and_data),
        )
            .abi_encode_params(),
    );
    keccak256((inner, entry_point, U256::from(chain_id)).abi_encode_params())
}

/// `initCode = factory ‖ factoryData` (empty when there's no factory —
/// always the case for an EIP-7702-delegated account, which isn't
/// CREATE2-deployed).
fn pack_init_code(op: &PackedUserOperation) -> Bytes {
    match op.factory {
        Some(factory) => {
            let mut v = factory.as_slice().to_vec();
            if let Some(data) = &op.factory_data {
                v.extend_from_slice(data);
            }
            Bytes::from(v)
        }
        None => Bytes::new(),
    }
}

/// `paymasterAndData = paymaster ‖ paymasterVerificationGasLimit(16) ‖
/// paymasterPostOpGasLimit(16) ‖ paymasterData` (empty when there's no
/// paymaster).
fn pack_paymaster_and_data(op: &PackedUserOperation) -> Bytes {
    match op.paymaster {
        Some(paymaster) => {
            let mut v = paymaster.as_slice().to_vec();
            let pvgl = op.paymaster_verification_gas_limit.unwrap_or(U256::ZERO);
            let ppogl = op.paymaster_post_op_gas_limit.unwrap_or(U256::ZERO);
            // Low 16 bytes of each — these are uint128 fields in the
            // packed layout.
            v.extend_from_slice(&pvgl.to_be_bytes::<32>()[16..]);
            v.extend_from_slice(&ppogl.to_be_bytes::<32>()[16..]);
            if let Some(data) = &op.paymaster_data {
                v.extend_from_slice(data);
            }
            Bytes::from(v)
        }
        None => Bytes::new(),
    }
}

/// Pack two `uint128`-range values into one `bytes32`: `high` in the
/// upper 16 bytes, `low` in the lower 16. Matches EntryPoint v0.7's
/// `accountGasLimits` / `gasFees` layout. Inputs above `u128::MAX` are
/// masked, mirroring the on-chain `uint128(...)` truncation.
fn pack_u128_pair(high: U256, low: U256) -> B256 {
    let mask = U256::from(u128::MAX);
    let packed: U256 = ((high & mask) << 128) | (low & mask);
    B256::from(packed.to_be_bytes::<32>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    /// Canonical EntryPoint v0.7 — same address on every chain.
    const ENTRY_POINT: Address = address!("0000000071727De22E5E9d8BAf0edAc6f37da032");
    const ARBITRUM: u64 = 42161;

    /// A fully-populated sample op. Includes a factory + paymaster so
    /// the packing paths are exercised; individual tests clone-and-mutate.
    fn sample_op() -> PackedUserOperation {
        PackedUserOperation {
            sender: address!("1111111111111111111111111111111111111111"),
            nonce: U256::from(7u64),
            factory: Some(address!("3333333333333333333333333333333333333333")),
            factory_data: Some(Bytes::from_static(&[0xfa, 0xce])),
            call_data: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
            call_gas_limit: U256::from(100_000u64),
            verification_gas_limit: U256::from(200_000u64),
            pre_verification_gas: U256::from(21_000u64),
            max_fee_per_gas: U256::from(1_000_000_000u64),
            max_priority_fee_per_gas: U256::from(1_000_000u64),
            paymaster: Some(address!("4444444444444444444444444444444444444444")),
            paymaster_verification_gas_limit: Some(U256::from(50_000u64)),
            paymaster_post_op_gas_limit: Some(U256::from(40_000u64)),
            paymaster_data: Some(Bytes::from_static(&[0xca, 0xfe])),
            signature: Bytes::from_static(&[0xaa]),
        }
    }

    #[test]
    fn deterministic() {
        let op = sample_op();
        assert_eq!(
            user_op_hash(&op, ENTRY_POINT, ARBITRUM),
            user_op_hash(&op, ENTRY_POINT, ARBITRUM),
        );
    }

    #[test]
    fn signature_is_excluded_from_hash() {
        let mut op = sample_op();
        let before = user_op_hash(&op, ENTRY_POINT, ARBITRUM);
        op.signature = Bytes::from_static(&[0xff, 0xff, 0xff, 0xff]);
        let after = user_op_hash(&op, ENTRY_POINT, ARBITRUM);
        assert_eq!(before, after, "signature must not affect userOpHash");
    }

    #[test]
    fn sensitive_to_each_field() {
        let base = user_op_hash(&sample_op(), ENTRY_POINT, ARBITRUM);

        let mutate = |f: &dyn Fn(&mut PackedUserOperation)| {
            let mut op = sample_op();
            f(&mut op);
            user_op_hash(&op, ENTRY_POINT, ARBITRUM)
        };

        assert_ne!(mutate(&|o| o.sender = Address::ZERO), base, "sender");
        assert_ne!(mutate(&|o| o.nonce = U256::from(8u64)), base, "nonce");
        assert_ne!(
            mutate(&|o| o.factory = Some(Address::ZERO)),
            base,
            "factory",
        );
        assert_ne!(
            mutate(&|o| o.factory_data = Some(Bytes::from_static(&[0x00]))),
            base,
            "factory_data",
        );
        assert_ne!(
            mutate(&|o| o.call_data = Bytes::from_static(&[0xde, 0xad])),
            base,
            "call_data",
        );
        assert_ne!(
            mutate(&|o| o.call_gas_limit = U256::from(100_001u64)),
            base,
            "call_gas_limit",
        );
        assert_ne!(
            mutate(&|o| o.verification_gas_limit = U256::from(200_001u64)),
            base,
            "verification_gas_limit",
        );
        assert_ne!(
            mutate(&|o| o.pre_verification_gas = U256::from(21_001u64)),
            base,
            "pre_verification_gas",
        );
        assert_ne!(
            mutate(&|o| o.max_fee_per_gas = U256::from(1_000_000_001u64)),
            base,
            "max_fee_per_gas",
        );
        assert_ne!(
            mutate(&|o| o.max_priority_fee_per_gas = U256::from(1_000_001u64)),
            base,
            "max_priority_fee_per_gas",
        );
        assert_ne!(
            mutate(&|o| o.paymaster = Some(Address::ZERO)),
            base,
            "paymaster",
        );
        assert_ne!(
            mutate(&|o| o.paymaster_verification_gas_limit = Some(U256::from(50_001u64))),
            base,
            "paymaster_verification_gas_limit",
        );
        assert_ne!(
            mutate(&|o| o.paymaster_post_op_gas_limit = Some(U256::from(40_001u64))),
            base,
            "paymaster_post_op_gas_limit",
        );
        assert_ne!(
            mutate(&|o| o.paymaster_data = Some(Bytes::from_static(&[0x00]))),
            base,
            "paymaster_data",
        );
    }

    /// The `accountGasLimits` / `gasFees` packing must put `high` in the
    /// upper 16 bytes and `low` in the lower 16 — swapping the pair must
    /// change the hash, which proves we didn't transpose them.
    #[test]
    fn packing_order_is_not_transposed() {
        let base = user_op_hash(&sample_op(), ENTRY_POINT, ARBITRUM);

        let mut swapped_gas_limits = sample_op();
        std::mem::swap(
            &mut swapped_gas_limits.call_gas_limit,
            &mut swapped_gas_limits.verification_gas_limit,
        );
        assert_ne!(
            user_op_hash(&swapped_gas_limits, ENTRY_POINT, ARBITRUM),
            base,
            "call/verification gas limits must occupy distinct halves",
        );

        let mut swapped_fees = sample_op();
        std::mem::swap(
            &mut swapped_fees.max_fee_per_gas,
            &mut swapped_fees.max_priority_fee_per_gas,
        );
        assert_ne!(
            user_op_hash(&swapped_fees, ENTRY_POINT, ARBITRUM),
            base,
            "max-fee / max-priority-fee must occupy distinct halves",
        );
    }

    #[test]
    fn sensitive_to_entry_point_and_chain() {
        let op = sample_op();
        let base = user_op_hash(&op, ENTRY_POINT, ARBITRUM);
        assert_ne!(
            user_op_hash(
                &op,
                address!("0000000000000000000000000000000000000001"),
                ARBITRUM
            ),
            base,
        );
        assert_ne!(user_op_hash(&op, ENTRY_POINT, 1), base);
    }

    /// `pack_u128_pair` must place `high` in the top 16 bytes, `low` in
    /// the bottom 16 — checked against a hand-computed `bytes32`.
    #[test]
    fn pack_u128_pair_layout() {
        let packed = pack_u128_pair(U256::from(0xAAu64), U256::from(0xBBu64));
        let mut expected = [0u8; 32];
        expected[15] = 0xAA; // high half: byte 15 is the LSB of the upper u128
        expected[31] = 0xBB; // low half: byte 31 is the LSB of the lower u128
        assert_eq!(packed, B256::from(expected));
    }
}
