//! ERC-4337 bundler RPC, EIP-7702-aware.
//!
//! Three of the four bundler methods we use are first-party
//! ([`Erc4337Api::estimate_user_operation_gas`],
//! [`Erc4337Api::send_user_operation`],
//! [`Erc4337Api::get_user_operation_receipt`],
//! [`Erc4337Api::supported_entry_points`] from `alloy-provider`),
//! BUT `estimate_user_operation_gas` + `send_user_operation` are
//! useless to us as-is: EntryPoint v0.7's `PackedUserOperation` has no
//! `eip7702Auth` field, and alloy's `SendUserOperation` enum doesn't
//! carry one either. Bundlers accept the signed 7702 authorization as
//! an *extra field on the userOperation JSON object* — outside the
//! ERC-4337 spec, a bundler-RPC extension every major bundler
//! implements.
//!
//! So this module hand-rolls the two userOp-carrying methods to inject
//! `eip7702Auth`, and delegates the other two to alloy.
//!
//! ## Field-casing variance
//!
//! The `eip7702Auth` object's *inner* field names differ by bundler:
//!
//! | Bundler        | Casing      | `v` field |
//! |----------------|-------------|-----------|
//! | Pimlico        | camelCase   | yes       |
//! | ZeroDev        | camelCase   | no        |
//! | Alchemy        | snake_case  | no        |
//!
//! [`BundlerCasing`] selects between them; we never emit `v` since the
//! signed-auth shape we accept doesn't carry it (it's redundant with
//! `yParity` and bundlers that read both still accept `yParity`-only).

use crate::error::Error;
use crate::error::Result;
use alloy::primitives::Address;
use alloy::primitives::B256;
use alloy::primitives::U256;
use alloy::providers::Provider;
use alloy::providers::RootProvider;
use alloy::rpc::types::erc4337::PackedUserOperation;
use alloy_provider::ext::Erc4337Api;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use url::Url;

/// Bundler-specific JSON casing for the `eip7702Auth` object's *inner*
/// field names. Defaults to [`Self::CamelCase`] — what ZeroDev and
/// Pimlico both accept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum BundlerCasing {
    /// `chainId`, `yParity`, … — Pimlico, ZeroDev.
    #[default]
    #[serde(rename = "camel")]
    CamelCase,
    /// `chain_id`, `y_parity`, … — Alchemy.
    #[serde(rename = "snake")]
    SnakeCase,
}

/// `eth_estimateUserOperationGas` response shape, matching the
/// ERC-4337 v0.7 spec field names. Defined here (rather than reusing
/// alloy's `UserOperationGasEstimation`) because alloy mistakenly names
/// the verification field `verificationGas` — every bundler returns
/// `verificationGasLimit`, and the alloy shape fails to deserialize.
///
/// Paymaster gas fields are `Option` since they're only present when
/// the userOp carried `paymaster*` fields in the request.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GasEstimation {
    pub pre_verification_gas: U256,
    pub verification_gas_limit: U256,
    pub call_gas_limit: U256,
    #[serde(default)]
    pub paymaster_verification_gas_limit: Option<U256>,
    #[serde(default)]
    pub paymaster_post_op_gas_limit: Option<U256>,
}

/// Minimal `eth_getUserOperationReceipt` response shape.
///
/// Hand-rolled (rather than reusing alloy's `UserOperationReceipt`)
/// because alloy declares `paymaster` and `reason` as non-optional
/// fields, whereas ERC-4337 v0.7 (and every bundler) omits / nulls
/// them for unsponsored ops and on success respectively. Decoding then
/// fails with an error whose message contains `"null"`, which our
/// pending-detection heuristic used to swallow as `Ok(None)` — turning
/// every successful receipt into a missed poll.
///
/// We only consume `receipt.transaction_hash` downstream, so anything
/// else is intentionally absent. Add fields here when callers grow new
/// needs — don't reach for alloy's bloated shape.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserOperationReceipt {
    pub receipt: InnerTxReceipt,
}

/// Just the tx-hash slice of the embedded transaction receipt.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InnerTxReceipt {
    pub transaction_hash: B256,
}

/// Signed EIP-7702 authorization in the flat shape bundlers expect.
///
/// Decoupled from `alloy-eip7702`'s nested `SignedAuthorization` so the
/// orchestration layer (Phase 5) is free to populate this from any
/// signing path (`alloy::primitives::Signature`, raw `k256`, etc.)
/// without leaking that choice into the bundler API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedEip7702Authorization {
    pub chain_id: u64,
    /// The delegation target — for us, the Kernel V3.3 implementation.
    pub address: Address,
    /// The EOA's account nonce at the time of signing (separate from
    /// the userOp nonce — this is `eth_getTransactionCount(addr)`).
    pub nonce: u64,
    pub y_parity: u8,
    pub r: B256,
    pub s: B256,
}

/// Thin wrapper over an alloy provider that speaks bundler RPC.
///
/// One [`BundlerClient`] = one bundler endpoint + a chosen [`BundlerCasing`].
pub struct BundlerClient {
    provider: RootProvider,
    casing: BundlerCasing,
}

impl BundlerClient {
    /// Connect to `url` with the given `casing`.
    ///
    /// Uses a bare [`RootProvider`] (no fillers) — bundler RPC never
    /// needs the gas / nonce / chain-id fillers `ProviderBuilder::new`
    /// installs by default, and avoiding them keeps the type signature
    /// stable across alloy minor versions.
    pub fn new(url: Url, casing: BundlerCasing) -> Result<Self> {
        let provider = RootProvider::new_http(url);
        Ok(Self { provider, casing })
    }

    /// `eth_estimateUserOperationGas`. Hand-rolled because alloy's
    /// `estimate_user_operation_gas` can't attach `eip7702Auth`, which
    /// bundlers need to simulate against the (about-to-be-delegated)
    /// EOA's code rather than its current empty bytecode.
    ///
    /// We also don't use alloy's `UserOperationGasEstimation` response
    /// type because it names the field `verification_gas`
    /// (`verificationGas` on the wire) — the actual ERC-4337 v0.7 spec
    /// (and every bundler) returns `verificationGasLimit`, and the
    /// alloy shape fails to deserialize. Local [`GasEstimation`] matches
    /// the wire.
    pub async fn estimate_user_operation_gas(
        &self,
        userop: &PackedUserOperation,
        entry_point: Address,
        eip7702_auth: Option<&SignedEip7702Authorization>,
    ) -> Result<GasEstimation> {
        let userop_json = userop_with_auth_json(userop, eip7702_auth, self.casing)?;
        self.provider
            .client()
            .request("eth_estimateUserOperationGas", (userop_json, entry_point))
            .await
            .map_err(|e| Error::Transport(format!("eth_estimateUserOperationGas: {e}")))
    }

    /// `eth_sendUserOperation` → returns the bundler-computed
    /// `userOpHash`. Hand-rolled for the same reason as
    /// [`Self::estimate_user_operation_gas`].
    pub async fn send_user_operation(
        &self,
        userop: &PackedUserOperation,
        entry_point: Address,
        eip7702_auth: Option<&SignedEip7702Authorization>,
    ) -> Result<B256> {
        let userop_json = userop_with_auth_json(userop, eip7702_auth, self.casing)?;
        self.provider
            .client()
            .request("eth_sendUserOperation", (userop_json, entry_point))
            .await
            .map_err(|e| Error::Transport(format!("eth_sendUserOperation: {e}")))
    }

    /// `eth_getUserOperationReceipt`. Returns `None` while the op is
    /// still pending — typed as `Option<_>` so a bundler-side `null`
    /// deserializes cleanly without any string-matching on error
    /// messages (that heuristic used to mistake a non-null receipt
    /// containing a null field for "pending"; see [`UserOperationReceipt`]).
    pub async fn get_user_operation_receipt(
        &self,
        user_op_hash: B256,
    ) -> Result<Option<UserOperationReceipt>> {
        self.provider
            .client()
            .request("eth_getUserOperationReceipt", (user_op_hash,))
            .await
            .map_err(|e| Error::Transport(format!("eth_getUserOperationReceipt: {e}")))
    }

    /// `eth_supportedEntryPoints`. Useful for a startup sanity check —
    /// the configured EntryPoint v0.7 address should appear here.
    pub async fn supported_entry_points(&self) -> Result<Vec<Address>> {
        self.provider
            .supported_entry_points()
            .await
            .map_err(|e| Error::Transport(format!("eth_supportedEntryPoints: {e}")))
    }
}

// ── helpers ────────────────────────────────────────────────────────────

/// Serialise the userOp to its JSON-RPC shape and, if `auth` is set,
/// inject the `eip7702Auth` object with the chosen casing.
///
/// `pub(crate)` so the paymaster module can build the same userOp shape
/// for its `pm_*` calls — bundler and paymaster speak the same dialect.
pub(crate) fn userop_with_auth_json(
    userop: &PackedUserOperation,
    auth: Option<&SignedEip7702Authorization>,
    casing: BundlerCasing,
) -> Result<Value> {
    // We only ever emit EntryPoint v0.7 userOps, so we serialize the
    // alloy struct directly rather than via `SendUserOperation` (which
    // is an externally-tagged enum that would wrap us in `{"EntryPointV07":…}`).
    let mut value = serde_json::to_value(userop)
        .map_err(|e| Error::Transport(format!("serialise userOp: {e}")))?;
    if let Some(auth) = auth {
        let obj = value
            .as_object_mut()
            .ok_or_else(|| Error::Transport("serialised userOp is not an object".to_string()))?;
        obj.insert("eip7702Auth".to_string(), auth_to_json(auth, casing));
    }
    Ok(value)
}

/// Build the `eip7702Auth` object with the given field casing.
///
/// All integer fields use the JSON-RPC quantity convention (hex without
/// leading zeros); `address`, `r`, `s` use alloy's default serde
/// (fixed-width hex preserving leading zeros) which matches what every
/// bundler accepts.
fn auth_to_json(auth: &SignedEip7702Authorization, casing: BundlerCasing) -> Value {
    match casing {
        BundlerCasing::CamelCase => json!({
            "chainId": format!("{:#x}", auth.chain_id),
            "address": auth.address,
            "nonce":   format!("{:#x}", auth.nonce),
            "yParity": format!("{:#x}", auth.y_parity),
            "r":       auth.r,
            "s":       auth.s,
        }),
        BundlerCasing::SnakeCase => json!({
            "chain_id": format!("{:#x}", auth.chain_id),
            "address":  auth.address,
            "nonce":    format!("{:#x}", auth.nonce),
            "y_parity": format!("{:#x}", auth.y_parity),
            "r":        auth.r,
            "s":        auth.s,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Bytes;
    use alloy::primitives::U256;
    use alloy::primitives::address;
    use alloy::primitives::b256;

    fn sample_userop() -> PackedUserOperation {
        PackedUserOperation {
            sender: address!("1111111111111111111111111111111111111111"),
            nonce: U256::from(7u64),
            factory: None,
            factory_data: None,
            call_data: Bytes::from_static(&[0xde, 0xad]),
            call_gas_limit: U256::from(100_000u64),
            verification_gas_limit: U256::from(200_000u64),
            pre_verification_gas: U256::from(21_000u64),
            max_fee_per_gas: U256::from(1_000_000_000u64),
            max_priority_fee_per_gas: U256::from(1_000_000u64),
            paymaster: None,
            paymaster_verification_gas_limit: None,
            paymaster_post_op_gas_limit: None,
            paymaster_data: None,
            signature: Bytes::from_static(&[0xaa]),
        }
    }

    fn sample_auth() -> SignedEip7702Authorization {
        SignedEip7702Authorization {
            chain_id: 42161,
            address: address!("d6CEDDe84be40893d153Be9d467CD6aD37875b28"),
            nonce: 5,
            y_parity: 1,
            r: b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            s: b256!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        }
    }

    #[test]
    fn userop_without_auth_omits_field() {
        let v =
            userop_with_auth_json(&sample_userop(), None, BundlerCasing::CamelCase).expect("ok");
        assert!(v.get("eip7702Auth").is_none(), "no auth field when None");
        // The userOp body itself is still present + camelCase.
        assert!(v.get("sender").is_some(), "userOp body intact");
        assert!(v.get("callData").is_some(), "userOp fields are camelCase");
    }

    #[test]
    fn camelcase_auth_shape() {
        let v = userop_with_auth_json(
            &sample_userop(),
            Some(&sample_auth()),
            BundlerCasing::CamelCase,
        )
        .expect("ok");
        let auth = v.get("eip7702Auth").expect("auth attached").clone();

        assert_eq!(auth["chainId"], "0xa4b1");
        assert_eq!(auth["nonce"], "0x5");
        assert_eq!(auth["yParity"], "0x1");
        // address + r + s use alloy's default serde (0x-prefixed hex of
        // exactly the right width).
        assert_eq!(
            auth["address"].as_str().unwrap().to_lowercase(),
            "0xd6cedde84be40893d153be9d467cd6ad37875b28",
        );
        assert_eq!(
            auth["r"],
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        assert_eq!(
            auth["s"],
            "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        assert!(auth.get("v").is_none(), "no v field in our shape");
        assert!(
            auth.get("chain_id").is_none(),
            "snake_case fields not emitted under CamelCase",
        );
    }

    #[test]
    fn snakecase_auth_shape() {
        let v = userop_with_auth_json(
            &sample_userop(),
            Some(&sample_auth()),
            BundlerCasing::SnakeCase,
        )
        .expect("ok");
        let auth = v.get("eip7702Auth").expect("auth attached");

        assert_eq!(auth["chain_id"], "0xa4b1");
        assert_eq!(auth["y_parity"], "0x1");
        assert_eq!(auth["nonce"], "0x5");
        assert!(
            auth.get("chainId").is_none(),
            "camelCase fields not emitted under SnakeCase",
        );
        assert!(auth.get("yParity").is_none());
        // Outer field name is always camelCase per the bundler docs.
        assert!(
            v.get("eip7702Auth").is_some() && v.get("eip7702_auth").is_none(),
            "outer field is always `eip7702Auth` regardless of inner casing",
        );
    }

    #[test]
    fn quantity_fields_strip_leading_zeros() {
        // chainId=1 -> "0x1" not "0x00000001".
        let auth = SignedEip7702Authorization {
            chain_id: 1,
            nonce: 0,
            y_parity: 0,
            ..sample_auth()
        };
        let v = userop_with_auth_json(&sample_userop(), Some(&auth), BundlerCasing::CamelCase)
            .expect("ok");
        let auth_json = v.get("eip7702Auth").unwrap();
        assert_eq!(auth_json["chainId"], "0x1");
        assert_eq!(auth_json["nonce"], "0x0");
        assert_eq!(auth_json["yParity"], "0x0");
    }
}
