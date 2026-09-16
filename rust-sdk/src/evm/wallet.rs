use super::*;
use alloy::network::EthereumWallet;
use alloy::network::TransactionBuilder;
use alloy::providers::DynProvider;
use alloy::providers::Provider;
use alloy::providers::ProviderBuilder;
use alloy::rpc::types::BlockNumberOrTag;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::SignerSync;
use alloy::signers::local::PrivateKeySigner;
use std::time::Duration;

/// Local signing key plus an RPC reader/sender. Its debug output omits key and RPC URL.
pub struct RpcWallet {
    provider: DynProvider,
    signer: PrivateKeySigner,
    chain_id: u64,
}
impl std::fmt::Debug for RpcWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcWallet")
            .field("address", &self.signer.address())
            .field("chain_id", &self.chain_id)
            .finish_non_exhaustive()
    }
}
impl RpcWallet {
    pub async fn connect(
        key: &crate::signer::EvmKey,
        chain_id: u64,
        url: url::Url,
    ) -> Result<Self> {
        let signer = PrivateKeySigner::from_slice(&key.secret_key)
            .map_err(|e| Error::InvalidSigner(e.to_string()))?;
        let provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(signer.clone()))
            .connect_http(url)
            .erased();
        if provider.get_chain_id().await.map_err(transport)? != chain_id {
            return Err(invalid("RPC chain does not match wallet"));
        }
        Ok(Self {
            provider,
            signer,
            chain_id,
        })
    }
    fn request(&self, tx: &EvmTransaction) -> Result<TransactionRequest> {
        if tx.chain_id != self.chain_id || tx.from != self.signer.address() {
            return Err(invalid("transaction chain/sender mismatch"));
        }
        Ok(TransactionRequest::default()
            .from(tx.from)
            .to(tx.to)
            .value(tx.value)
            .input(tx.data.clone().into())
            .with_chain_id(self.chain_id))
    }
}
fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}
impl EvmWallet for RpcWallet {
    fn address(&self) -> Address {
        self.signer.address()
    }
    fn chain_id(&self) -> u64 {
        self.chain_id
    }
    fn sign_typed_data<'a>(&'a self, data: &'a EvmTypedData) -> WalletFuture<'a, Signature> {
        Box::pin(async move {
            let typed: alloy_dyn_abi::TypedData = serde_json::from_value(data.payload.clone())
                .map_err(|e| Error::Decode(e.to_string()))?;
            let digest = typed
                .eip712_signing_hash()
                .map_err(|e| Error::Decode(e.to_string()))?;
            if digest != data.digest {
                return Err(invalid("typed data and digest do not match"));
            }
            self.signer
                .sign_hash_sync(&digest)
                .map_err(|e| Error::InvalidSigner(e.to_string()))
        })
    }
    fn call<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, Bytes> {
        Box::pin(async move {
            self.provider
                .call(self.request(tx)?)
                .await
                .map_err(transport)
        })
    }
    fn send_transaction<'a>(&'a self, tx: &'a EvmTransaction) -> WalletFuture<'a, B256> {
        Box::pin(async move {
            Ok(*self
                .provider
                .send_transaction(self.request(tx)?)
                .await
                .map_err(transport)?
                .tx_hash())
        })
    }
    fn wait_for_receipt(&self, hash: B256) -> WalletFuture<'_, EvmReceipt> {
        Box::pin(async move {
            tokio::time::timeout(Duration::from_secs(120), async {
                loop {
                    if let Some(receipt) = self
                        .provider
                        .get_transaction_receipt(hash)
                        .await
                        .map_err(transport)?
                    {
                        return Ok(EvmReceipt {
                            transaction_hash: receipt.transaction_hash,
                            success: receipt.status(),
                            block_number: receipt
                                .block_number
                                .ok_or_else(|| invalid("receipt has no mined block"))?,
                            logs: receipt
                                .logs()
                                .iter()
                                .map(|log| EvmLog {
                                    address: log.address(),
                                    topics: log.topics().to_vec(),
                                    data: log.data().data.clone(),
                                })
                                .collect(),
                        });
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            })
            .await
            .map_err(|_| Error::Timeout(format!("receipt for {hash}")))?
        })
    }
    fn block_timestamp(&self) -> WalletFuture<'_, u64> {
        Box::pin(async {
            let block = self
                .provider
                .get_block_by_number(BlockNumberOrTag::Latest)
                .await
                .map_err(transport)?
                .ok_or_else(|| invalid("latest block unavailable"))?;
            Ok(block.header.timestamp)
        })
    }
}
