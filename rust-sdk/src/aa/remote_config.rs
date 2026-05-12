//! Fetcher for the backend's `GET /aa/config?chain={id}` endpoint.
//!
//! Lets SDK consumers avoid hard-coding bundler URLs (which embed a
//! provider API key we own). Returns just the wire-shaped data; the
//! caller composes it into the full [`crate::aa::AaConfig`] alongside
//! their own node RPC + (optional) paymaster context + gas overrides.

use crate::aa::bundler::BundlerCasing;
use crate::aa::client_ext::AaConfig;
use crate::aa::client_ext::GasOverrides;
use crate::aa::client_ext::PaymasterConfig;
use crate::request::Endpoint;
use crate::request::PayloadKind;
use crate::types::Chain;
use alloy::primitives::Address;
use reqwest::Method;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use url::Url;

/// Query parameters for `GET /aa/config`. Serializes to `?chain={id}`,
/// using [`Chain`]'s wire encoding (`"42161"` for Arbitrum, etc).
#[derive(Clone, Debug, Serialize)]
pub struct AaConfigRequest {
    pub chain: Chain,
}

impl AaConfigRequest {
    pub fn new(chain: Chain) -> Self {
        Self { chain }
    }
}

impl Endpoint for AaConfigRequest {
    type Response = RemoteAaConfig;
    const METHOD: Method = Method::GET;
    const PATH: &'static str = "aa/config";
    const PAYLOAD: PayloadKind = PayloadKind::Query;
}

/// Wire response of `GET /aa/config`. Mirrors the backend's
/// `AaConfigResponse` exactly.
///
/// The endpoint is intentionally absent from the backend's OpenAPI spec
/// — it's a private surface for our own SDK clients — so this shape
/// has no generated counterpart. Keep this struct in lockstep with
/// `swap/src/api/aa_config.rs::AaConfigResponse`.
#[derive(Clone, Debug, Deserialize)]
pub struct RemoteAaConfig {
    /// Canonical EntryPoint v0.7 deployment.
    pub entry_point: Address,
    /// Kernel V3.3 implementation contract the depositor's EOA delegates
    /// to via EIP-7702.
    pub delegation_target: Address,
    /// Canonical Permit2 deployment.
    pub permit2: Address,
    pub bundler: RemoteBundlerInfo,
    pub paymaster: Option<RemotePaymasterInfo>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RemoteBundlerInfo {
    pub url: Url,
    pub casing: BundlerCasing,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RemotePaymasterInfo {
    pub url: Url,
}

impl RemoteAaConfig {
    /// Compose into a full [`AaConfig`]. The remote response carries the
    /// bundler URL + casing and the (optional) paymaster URL; the caller
    /// supplies the EVM node RPC, paymaster context, and gas-limit
    /// overrides, since none of those are appropriate to ship from the
    /// server:
    /// - `node_rpc_url`: caller's own provider URL. For Alchemy you can pass
    ///   `self.bundler.url.clone()` — the same endpoint serves `eth_*` and `eth_sendUserOperation`.
    /// - `paymaster_context`: paymaster-specific (e.g. Alchemy Gas Manager wants `{"policyId":
    ///   "<uuid>"}`). Use `Value::Null` when the paymaster doesn't take one.
    /// - `gas_overrides`: pass `Some(_)` to bypass `eth_estimateUserOperationGas` (see
    ///   [`AaConfig::gas_overrides`]).
    pub fn into_config(
        self,
        node_rpc_url: Url,
        paymaster_context: Value,
        gas_overrides: Option<GasOverrides>,
    ) -> AaConfig {
        let paymaster = self.paymaster.map(|p| PaymasterConfig {
            url: p.url,
            context: paymaster_context,
        });
        AaConfig {
            bundler_url: self.bundler.url,
            node_rpc_url,
            paymaster,
            bundler_casing: self.bundler.casing,
            gas_overrides,
        }
    }
}
