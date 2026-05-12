//! Permit2 `PermitWitnessTransferFrom` EIP-712 typed-data + digest.
//!
//! The HTLC coordinator's `executeAndCreateWithPermit2(...)` pulls
//! tokens from the depositor via Uniswap's Permit2. The depositor signs
//! a `PermitWitnessTransferFrom` whose `witness` binds the signature
//! to the specific swap parameters (preimage hash, lock token, claim
//! address, refund address, timelock, inner calls hash) — so Permit2
//! can only authorise this *one* swap, not a generic transfer.
//!
//! Two specifics about Permit2's EIP-712:
//! 1. Domain uses the **short form** `EIP712Domain(string name, uint256 chainId, address
//!    verifyingContract)` — no `version` field. alloy's `Eip712Domain` handles this by emitting the
//!    field only when `Some`.
//! 2. `EIP712Domain.name = "Permit2"` (literal), `verifyingContract` is the canonical Permit2
//!    deployment (same on every chain).
//!
//! Reference: `client-sdk/ts-pure-sdk/src/evm/coordinator.ts`
//! `buildPermit2TypedData` — the TS SDK's implementation we cross-test
//! against in Phase 5 e2e.

use alloy::primitives::Address;
use alloy::primitives::B256;
use alloy::primitives::U256;
use alloy::primitives::address;
use alloy::sol;
use alloy::sol_types::SolStruct;
use alloy::sol_types::eip712_domain;

/// Canonical Permit2 deployment — same address on every EVM chain.
pub const PERMIT2_ADDRESS: Address = address!("000000000022D473030F116dDEE9F6B43aC78BA3");

sol! {
    // Same shape as `abi::TokenPermissions`, redeclared here so this
    // module's `sol!` block can derive a `SolStruct` impl for the
    // EIP-712 typehash dependency graph. The two types share no Rust
    // identity but encode identically.
    #[derive(Debug, PartialEq, Eq)]
    struct TokenPermissions {
        address token;
        uint256 amount;
    }

    /// Witness type bound into the Permit2 signature. Locks the
    /// authorisation to the exact swap's HTLC parameters + the
    /// `keccak256(abi.encode(forward_calls))` from the backend, so
    /// re-using the signature against a different swap is impossible.
    #[derive(Debug, PartialEq, Eq)]
    struct ExecuteAndCreate {
        bytes32 preimageHash;
        address token;
        address claimAddress;
        address refundAddress;
        uint256 timelock;
        bytes32 callsHash;
    }

    /// Primary EIP-712 type the depositor signs.
    #[derive(Debug, PartialEq, Eq)]
    struct PermitWitnessTransferFrom {
        TokenPermissions permitted;
        address spender;
        uint256 nonce;
        uint256 deadline;
        ExecuteAndCreate witness;
    }
}

/// Inputs to the Permit2 witness signature for a given swap.
///
/// Maps 1:1 onto the backend's `UseropFundingCalldataResponse` fields
/// plus a fresh `nonce` and `deadline` the SDK picks.
#[derive(Debug, Clone)]
pub struct PermitWitnessParams {
    /// ERC-20 the depositor's smart account permits Permit2 to pull.
    pub source_token: Address,
    /// Smallest-unit amount of `source_token` to permit.
    pub source_amount: U256,
    /// Spender Permit2 authorises — the HTLC coordinator contract.
    pub coordinator_address: Address,
    /// Random `nonce` (full uint256) the depositor picks for this permit.
    pub nonce: U256,
    /// Unix-seconds expiry of the permit.
    pub deadline: U256,
    // ── witness fields ──
    /// `sha256(preimage)` — the HTLC lock.
    pub preimage_hash: B256,
    /// Token locked into the HTLC (typically WBTC).
    pub lock_token: Address,
    /// Server's claim address.
    pub claim_address: Address,
    /// Address that can refund — the coordinator itself.
    pub refund_address: Address,
    /// Unix-seconds HTLC timelock.
    pub timelock: U256,
    /// `keccak256(abi.encode(forward_calls))` — pins the DEX calls the
    /// backend supplied so the swap can't be reused with a different
    /// inner batch.
    pub calls_hash: B256,
}

impl PermitWitnessParams {
    fn into_typed(self) -> PermitWitnessTransferFrom {
        PermitWitnessTransferFrom {
            permitted: TokenPermissions {
                token: self.source_token,
                amount: self.source_amount,
            },
            spender: self.coordinator_address,
            nonce: self.nonce,
            deadline: self.deadline,
            witness: ExecuteAndCreate {
                preimageHash: self.preimage_hash,
                token: self.lock_token,
                claimAddress: self.claim_address,
                refundAddress: self.refund_address,
                timelock: self.timelock,
                callsHash: self.calls_hash,
            },
        }
    }
}

/// Compute the EIP-712 digest the depositor signs.
///
/// This is the `\x19\x01 || domainSeparator || structHash` digest, not
/// EIP-191. Signing it with the depositor's key produces the `signature`
/// passed to `executeAndCreateWithPermit2`.
pub fn permit2_digest(params: PermitWitnessParams, chain_id: u64) -> B256 {
    let permit = params.into_typed();
    let domain = eip712_domain! {
        name: "Permit2",
        chain_id: chain_id,
        verifying_contract: PERMIT2_ADDRESS,
    };
    permit.eip712_signing_hash(&domain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;
    use alloy::primitives::b256;

    /// Fresh sample params. Tests clone-and-mutate to exercise each
    /// field's contribution to the digest.
    fn sample_params() -> PermitWitnessParams {
        PermitWitnessParams {
            source_token: address!("af88d065e77c8cC2239327C5EDb3A432268e5831"), /* USDC.e on Arbitrum */
            source_amount: U256::from(1_000_000u64),
            coordinator_address: address!("1111111111111111111111111111111111111111"),
            nonce: U256::from(0x42u64),
            deadline: U256::from(1_800_000_000u64),
            preimage_hash: b256!(
                "0101010101010101010101010101010101010101010101010101010101010101"
            ),
            lock_token: address!("2f2a2543B76A4166549F7aAB2e75Bef0aefC5B0f"), // WBTC on Arbitrum
            claim_address: address!("2222222222222222222222222222222222222222"),
            refund_address: address!("1111111111111111111111111111111111111111"), // = coordinator
            timelock: U256::from(1_800_010_000u64),
            calls_hash: b256!("0202020202020202020202020202020202020202020202020202020202020202"),
        }
    }

    /// Cross-check the EIP-712 typehashes alloy produces against
    /// `cast keccak`-verified expected values.
    ///
    /// Pinned values are computed from:
    /// - `PermitWitnessTransferFrom(...)ExecuteAndCreate(...)TokenPermissions(...)` (referenced
    ///   types appended alphabetically per EIP-712)
    /// - `ExecuteAndCreate(...)` standalone
    /// - `TokenPermissions(...)` standalone
    #[test]
    fn typehashes_match_external_oracle() {
        assert_eq!(
            <PermitWitnessTransferFrom as SolStruct>::eip712_type_hash(
                &sample_params().into_typed()
            ),
            b256!("b0f9eb4b35584d3f7204eeec979cd796ec233e806b23cc43d76ea87a9869c91b"),
        );
        assert_eq!(
            <ExecuteAndCreate as SolStruct>::eip712_type_hash(&ExecuteAndCreate {
                preimageHash: B256::ZERO,
                token: Address::ZERO,
                claimAddress: Address::ZERO,
                refundAddress: Address::ZERO,
                timelock: U256::ZERO,
                callsHash: B256::ZERO,
            }),
            b256!("01f068030ecb4faa4fc19679afdcbe9c2619d263df5345b2b316a22b6ac0a346"),
        );
        assert_eq!(
            <TokenPermissions as SolStruct>::eip712_type_hash(&TokenPermissions {
                token: Address::ZERO,
                amount: U256::ZERO,
            }),
            b256!("618358ac3db8dc274f0cd8829da7e234bd48cd73c4a740aede1adec9846d06a1"),
        );
    }

    #[test]
    fn digest_is_deterministic() {
        let p = sample_params();
        assert_eq!(permit2_digest(p.clone(), 42161), permit2_digest(p, 42161),);
    }

    #[test]
    fn digest_depends_on_chain_id() {
        let p = sample_params();
        assert_ne!(permit2_digest(p.clone(), 42161), permit2_digest(p, 1),);
    }

    #[test]
    fn digest_sensitive_to_each_field() {
        let base = permit2_digest(sample_params(), 42161);

        let mutate = |f: &dyn Fn(&mut PermitWitnessParams)| {
            let mut p = sample_params();
            f(&mut p);
            permit2_digest(p, 42161)
        };

        // permit fields
        assert_ne!(
            mutate(&|p| p.source_token = Address::ZERO),
            base,
            "source_token"
        );
        assert_ne!(
            mutate(&|p| p.source_amount = U256::from(1_000_001u64)),
            base,
            "source_amount",
        );
        assert_ne!(
            mutate(&|p| p.coordinator_address = Address::ZERO),
            base,
            "coordinator_address (= spender)",
        );
        assert_ne!(mutate(&|p| p.nonce = U256::from(0x43u64)), base, "nonce");
        assert_ne!(
            mutate(&|p| p.deadline = U256::from(1_800_000_001u64)),
            base,
            "deadline",
        );

        // witness fields
        assert_ne!(
            mutate(&|p| p.preimage_hash = B256::ZERO),
            base,
            "preimage_hash",
        );
        assert_ne!(
            mutate(&|p| p.lock_token = Address::ZERO),
            base,
            "lock_token"
        );
        assert_ne!(
            mutate(&|p| p.claim_address = Address::ZERO),
            base,
            "claim_address",
        );
        assert_ne!(
            mutate(&|p| p.refund_address = Address::ZERO),
            base,
            "refund_address",
        );
        assert_ne!(
            mutate(&|p| p.timelock = U256::from(1_800_010_001u64)),
            base,
            "timelock",
        );
        assert_ne!(mutate(&|p| p.calls_hash = B256::ZERO), base, "calls_hash");
    }
}
