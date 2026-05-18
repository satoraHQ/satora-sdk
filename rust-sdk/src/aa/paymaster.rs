//! ERC-7677 paymaster RPC (`pm_getPaymasterStubData` / `pm_getPaymasterData`).
//!
//! Two-call dance Alchemy's Gas Manager (and other ERC-7677-conformant
//! paymasters) expect:
//!
//! 1. **`pm_getPaymasterStubData`** — before gas estimation. Returns placeholder paymaster fields
//!    shaped like the real ones (correct byte lengths, plausible gas limits) so the bundler can
//!    simulate accurately. The result may carry `isFinal: true`, in which case step 2 is
//!    unnecessary.
//! 2. **`pm_getPaymasterData`** — after gas estimation, with the estimated gas values filled in.
//!    Returns the real, signed paymaster fields.
//!
//! For EntryPoint v0.7 both calls return the *split* shape
//! `{ paymaster, paymasterVerificationGasLimit, paymasterPostOpGasLimit,
//! paymasterData }`. (v0.6 uses a single `paymasterAndData` blob; we
//! don't speak v0.6.)
//!
//! The paymaster RPC may run on the same HTTP endpoint as the bundler
//! (Alchemy) or a separate one (many setups). [`PaymasterClient`] wraps
//! its own URL so the orchestration can configure them independently.
//!
//! Like [`crate::aa::bundler`], the userOp goes out *with* the
//! `eip7702Auth` field attached when supplied — paymasters need it to
//! simulate against the delegated Kernel code rather than the EOA's
//! current empty bytecode.

use crate::aa::bundler::BundlerCasing;
use crate::aa::bundler::SignedEip7702Authorization;
use crate::error::Error;
use crate::error::Result;
use alloy::primitives::Address;
use alloy::primitives::Bytes;
use alloy::primitives::U256;
use alloy::providers::Provider;
use alloy::providers::RootProvider;
use alloy::rpc::types::erc4337::PackedUserOperation;
use serde::Deserialize;
use serde_json::Value;
use url::Url;

/// Response shape for both `pm_getPaymasterStubData` and
/// `pm_getPaymasterData` under EntryPoint v0.7 — the *split* form.
///
/// `pm_getPaymasterStubData` may additionally include `is_final: true`
/// (signal that the stub data IS the final data and step 2 can be
/// skipped); we surface that flag so the caller can short-circuit.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PaymasterFields {
    pub paymaster: Address,
    pub paymaster_verification_gas_limit: U256,
    pub paymaster_post_op_gas_limit: U256,
    pub paymaster_data: Bytes,
    /// Only meaningful on the stub-data response: when `true`, this
    /// stub IS the final data and `pm_getPaymasterData` can be skipped.
    #[serde(default)]
    pub is_final: bool,
}

/// Paymaster RPC client.
pub struct PaymasterClient {
    provider: RootProvider,
    /// Matches the bundler casing for the embedded `eip7702Auth` object
    /// so the call site doesn't need two separate casing knobs. The
    /// outer paymaster RPC fields are spec-defined and never vary.
    casing: BundlerCasing,
}

impl PaymasterClient {
    /// Connect to `url` with the given inner-auth `casing` (matches the
    /// bundler's setting — the paymaster talks the same dialect as the
    /// bundler it sits in front of).
    pub fn new(url: Url, casing: BundlerCasing) -> Result<Self> {
        let provider = RootProvider::new_http(url);
        Ok(Self { provider, casing })
    }

    /// `pm_getPaymasterStubData` — call this BEFORE
    /// `eth_estimateUserOperationGas`.
    ///
    /// `context` is paymaster-specific JSON; for Alchemy Gas Manager
    /// it's `{ "policyId": "<uuid>" }`. Pass `Value::Null` if your
    /// paymaster doesn't take one.
    pub async fn get_paymaster_stub_data(
        &self,
        userop: &PackedUserOperation,
        entry_point: Address,
        chain_id: u64,
        context: Value,
        eip7702_auth: Option<&SignedEip7702Authorization>,
    ) -> Result<PaymasterFields> {
        self.request(
            "pm_getPaymasterStubData",
            userop,
            entry_point,
            chain_id,
            context,
            eip7702_auth,
        )
        .await
    }

    /// `pm_getPaymasterData` — call this AFTER
    /// `eth_estimateUserOperationGas`, with the userOp's gas fields
    /// populated. Returns the real (non-stub) paymaster data the
    /// bundler will accept at `eth_sendUserOperation`.
    pub async fn get_paymaster_data(
        &self,
        userop: &PackedUserOperation,
        entry_point: Address,
        chain_id: u64,
        context: Value,
        eip7702_auth: Option<&SignedEip7702Authorization>,
    ) -> Result<PaymasterFields> {
        self.request(
            "pm_getPaymasterData",
            userop,
            entry_point,
            chain_id,
            context,
            eip7702_auth,
        )
        .await
    }

    /// Shared dispatcher for the two `pm_*` calls — the only difference
    /// between them is the method name and *when* the caller invokes
    /// them in the userOp lifecycle.
    async fn request(
        &self,
        method: &'static str,
        userop: &PackedUserOperation,
        entry_point: Address,
        chain_id: u64,
        context: Value,
        eip7702_auth: Option<&SignedEip7702Authorization>,
    ) -> Result<PaymasterFields> {
        let userop_json =
            crate::aa::bundler::userop_with_auth_json(userop, eip7702_auth, self.casing)?;
        self.provider
            .client()
            .request(
                method,
                (userop_json, entry_point, format!("{chain_id:#x}"), context),
            )
            .await
            .map_err(|e| Error::Transport(format!("{method}: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    /// `PaymasterFields` deserializes the EntryPoint-v0.7 split-form
    /// response shape correctly, with `isFinal` defaulting to false.
    #[test]
    fn paymaster_fields_deserialize_v07_split_form() {
        let json = serde_json::json!({
            "paymaster": "0xdeaddeaddeaddeaddeaddeaddeaddeaddeaddead",
            "paymasterVerificationGasLimit": "0x186a0",
            "paymasterPostOpGasLimit": "0xc350",
            "paymasterData": "0xcafebabe",
        });
        let fields: PaymasterFields = serde_json::from_value(json).expect("deserializes");
        assert_eq!(
            fields.paymaster,
            address!("deaddeaddeaddeaddeaddeaddeaddeaddeaddead"),
        );
        assert_eq!(
            fields.paymaster_verification_gas_limit,
            U256::from(100_000u64)
        );
        assert_eq!(fields.paymaster_post_op_gas_limit, U256::from(50_000u64));
        assert_eq!(&fields.paymaster_data[..], &[0xca, 0xfe, 0xba, 0xbe]);
        assert!(!fields.is_final, "missing isFinal must default to false");
    }

    /// Stub-data response with `isFinal: true` signals the caller can
    /// skip `pm_getPaymasterData`.
    #[test]
    fn paymaster_fields_honour_is_final() {
        let json = serde_json::json!({
            "paymaster": "0x0000000000000000000000000000000000000001",
            "paymasterVerificationGasLimit": "0x0",
            "paymasterPostOpGasLimit": "0x0",
            "paymasterData": "0x",
            "isFinal": true,
        });
        let fields: PaymasterFields = serde_json::from_value(json).expect("deserializes");
        assert!(fields.is_final);
    }
}
