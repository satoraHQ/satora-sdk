//! EVM execution and external-wallet integration (`gasless` feature).
use crate::Error;
use crate::Result;
pub use alloy::primitives::Address;
pub use alloy::primitives::B256;
pub use alloy::primitives::Bytes;
pub use alloy::primitives::Signature;
pub use alloy::primitives::U256;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub mod claim;
pub mod funding;
pub use claim::EvmClaimOptions;
pub use claim::RedeemCalldata;
pub mod htlc;
pub mod permit;
pub mod recovery;
pub mod refund;
pub use recovery::DepositRecovery;
pub use recovery::RefundedEvmBalance;
pub use refund::EvmSubmission;
pub use refund::RefundOptions;
mod wallet;
pub use funding::FundingOptions;
pub use funding::Permit2FundingData;
pub use wallet::RpcWallet;

pub type WalletFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// reqwest and alloy transports default to no timeout, so a peer that accepts a
/// request and never answers would hold an execution lock indefinitely.
pub(crate) const RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub(crate) fn rpc_client(url: url::Url) -> alloy::rpc::client::RpcClient {
    let http = reqwest::Client::builder()
        .timeout(RPC_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    alloy::rpc::client::RpcClient::new(alloy::transports::http::Http::with_client(http, url), false)
}
pub(crate) fn http_provider(url: url::Url) -> alloy::providers::RootProvider {
    alloy::providers::RootProvider::new(rpc_client(url))
}

#[derive(Clone, PartialEq, Eq)]
pub struct EvmTransaction {
    pub chain_id: u64,
    pub from: Address,
    pub to: Address,
    pub value: U256,
    pub data: Bytes,
}

impl std::fmt::Debug for EvmTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvmTransaction")
            .field("chain_id", &self.chain_id)
            .field("from", &self.from)
            .field("to", &self.to)
            .field("value", &self.value)
            .field("data", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvmLog {
    pub address: Address,
    pub topics: Vec<B256>,
    pub data: Bytes,
}

#[derive(Clone, Debug)]
pub struct EvmReceipt {
    pub transaction_hash: B256,
    pub success: bool,
    pub block_number: u64,
    pub logs: Vec<EvmLog>,
}

/// EIP-712 JSON for wallet APIs plus the locally computed digest.
/// May contain a claim preimage: diagnostics deliberately omit both fields.
#[derive(Clone)]
pub struct EvmTypedData {
    pub payload: Value,
    pub digest: B256,
}
impl std::fmt::Debug for EvmTypedData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EvmTypedData { [REDACTED] }")
    }
}

/// Adapt an external wallet and its chain reader. Implementations must submit on
/// `chain_id`, return complete receipt logs, and surface failed/replaced transactions.
/// Every signature is recovered locally before submission.
pub trait EvmWallet: Send + Sync {
    fn address(&self) -> Address;
    fn chain_id(&self) -> u64;
    fn sign_typed_data<'a>(&'a self, data: &'a EvmTypedData) -> WalletFuture<'a, Signature>;
    fn call<'a>(&'a self, transaction: &'a EvmTransaction) -> WalletFuture<'a, Bytes>;
    fn send_transaction<'a>(&'a self, transaction: &'a EvmTransaction) -> WalletFuture<'a, B256>;
    fn wait_for_receipt(&self, hash: B256) -> WalletFuture<'_, EvmReceipt>;
    /// Chain time is required for unilateral refunds. Funding-only adapters may omit it.
    fn block_timestamp(&self) -> WalletFuture<'_, u64> {
        Box::pin(async { Err(invalid("wallet adapter does not expose chain time")) })
    }
}

pub(super) fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidSwap(message.into())
}
pub(super) fn decimal(value: &str) -> Result<U256> {
    U256::from_str_radix(value, 10).map_err(|e| Error::Decode(e.to_string()))
}
pub(super) fn parse<T: std::str::FromStr>(value: &str) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|e: T::Err| Error::Decode(e.to_string()))
}
/// OpenZeppelin ECDSA and the daemon reject high-s even though alloy recovery accepts it,
/// so callers must encode the returned signature, not the wallet's.
pub(super) fn verify_signature(
    signature: Signature,
    digest: B256,
    address: Address,
) -> Result<Signature> {
    let signature = signature.normalized_s();
    let recovered = signature
        .recover_address_from_prehash(&digest)
        .map_err(|e| Error::InvalidSigner(e.to_string()))?;
    if recovered != address {
        return Err(Error::InvalidSigner(
            "signature does not match the required EVM owner".into(),
        ));
    }
    Ok(signature)
}

/// A lock under any other hash could only be opened by whoever chose that hash.
/// Lightning-destination swaps lock under the invoice's payment hash instead.
pub(crate) fn verify_derived_hash_lock(
    client: &crate::Client,
    stored: &crate::StoredSwap,
    context: &EvmContext,
) -> Result<()> {
    if matches!(
        stored.response,
        crate::types::GetSwapResponse::EvmToLightning(_)
    ) {
        return Ok(());
    }
    let signer = client
        .signer
        .as_ref()
        .ok_or_else(|| Error::InvalidSigner("funding requires the swap seed".into()))?;
    if B256::from(signer.derive_swap_params(stored.key_index)?.hash_lock) != context.hash {
        return Err(invalid(
            "swap hash lock does not match the seed-derived preimage",
        ));
    }
    Ok(())
}

/// Shared identity fields. Amounts used for refunds are discovered from the
/// funding receipt, since a DEX's actual locked amount can differ from the quote.
#[derive(Clone, Debug)]
pub(crate) struct EvmContext {
    pub chain_id: u64,
    pub owner: Address,
    pub server: Address,
    pub htlc: Address,
    pub hash: B256,
    pub timelock: u64,
    pub source_token: Address,
    pub is_source: bool,
    pub kind: crate::types::EvmHtlcKind,
}
impl EvmContext {
    pub fn from_response(response: &crate::types::GetSwapResponse) -> Result<Self> {
        use crate::types::GetSwapResponse::*;
        let is_source = matches!(
            response,
            EvmToArkade(_) | EvmToBitcoin(_) | EvmToLightning(_)
        );
        if !is_source
            && !matches!(
                response,
                BitcoinToEvm(_) | ArkadeToEvm(_) | LightningToEvm(_)
            )
        {
            return Err(invalid("swap has no EVM side"));
        }
        let value = serde_json::to_value(response).map_err(|e| Error::Decode(e.to_string()))?;
        let string = |field: &str| {
            value[field]
                .as_str()
                .ok_or_else(|| invalid(format!("missing {field}")))
        };
        let source_token: Address = parse(if is_source {
            value["source_token"]["token_id"]
                .as_str()
                .ok_or_else(|| invalid("missing source token"))?
        } else {
            string("wbtc_address")?
        })?;
        let kind: crate::types::EvmHtlcKind = match value.get("evm_htlc_kind") {
            Some(kind) => {
                serde_json::from_value(kind.clone()).map_err(|e| Error::Decode(e.to_string()))?
            }
            None => crate::types::EvmHtlcKind::Erc20,
        };
        // HTLCErc20 cannot lock the zero address and HTLCNative always reports it. A
        // record stored before the kind was kept would otherwise sign the ERC20 domain
        // and hand the server a preimage for a claim it then rejects.
        if !is_source && (kind == crate::types::EvmHtlcKind::Native) != source_token.is_zero() {
            return Err(invalid(
                "HTLC kind and lock asset disagree; refresh the swap",
            ));
        }
        Ok(Self {
            chain_id: value["evm_chain_id"]
                .as_u64()
                .ok_or_else(|| invalid("invalid EVM chain ID"))?,
            owner: parse(string("client_evm_address")?)?,
            server: parse(string("server_evm_address")?)?,
            htlc: parse(string("evm_htlc_address")?)?,
            hash: parse(
                value
                    .get("evm_hash_lock")
                    .and_then(Value::as_str)
                    .unwrap_or(string("hash_lock").unwrap_or("")),
            )?,
            timelock: value["evm_refund_locktime"]
                .as_u64()
                .ok_or_else(|| invalid("invalid refund timelock"))?,
            source_token,
            is_source,
            kind,
        })
    }
    pub fn validate_wallet(&self, wallet: &dyn EvmWallet) -> Result<()> {
        if wallet.chain_id() != self.chain_id || wallet.address() != self.owner {
            return Err(invalid("wallet chain or address does not match swap"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
