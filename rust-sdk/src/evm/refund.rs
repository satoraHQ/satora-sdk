use super::funding::matching_creation;
use super::funding::transaction;
use super::funding::word;
use super::htlc::Coordinator;
use super::htlc::EvmHtlc;
use super::htlc::Htlc;
use super::htlc::typed_data;
use super::htlc::verify_version;
use super::*;
use crate::Client;
use crate::SwapMetadata;
use alloy::primitives::keccak256;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolValue;
use reqwest::Method;
use serde_json::json;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct RefundOptions {
    pub htlc: Address,
    pub lock_token: Address,
    /// None for direct HTLC funding; Some for a coordinator-funded swap.
    pub coordinator: Option<Address>,
    pub timeout: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollabRefundParams {
    pub coordinator_address: String,
    pub server_signer_address: String,
    pub preimage_hash: String,
    pub amount: String,
    pub token: String,
    pub claim_address: String,
    pub timelock: u64,
    pub chain_id: u64,
    pub sweep_token: String,
    pub min_amount_out: String,
    pub calls_hash: String,
    pub coordinator_version: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvmSubmission {
    pub id: String,
    pub tx_hash: String,
    pub message: String,
}

impl Client {
    /// Read the actual locked amount from the persisted funding transaction,
    /// checking the emitting contract, sender, token, lock, and timelock.
    pub async fn get_funded_evm_htlc(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: &RefundOptions,
    ) -> Result<EvmHtlc> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        let context = EvmContext::from_response(&stored.response)?;
        context.validate_wallet(wallet)?;
        if !context.is_source || context.htlc != options.htlc {
            return Err(invalid("refund requires the configured EVM-source HTLC"));
        }
        let value =
            serde_json::to_value(&stored.response).map_err(|e| Error::Decode(e.to_string()))?;
        let hash = stored
            .metadata
            .evm_fund_txid
            .as_deref()
            .or_else(|| value["evm_fund_txid"].as_str())
            .ok_or_else(|| {
                invalid("funding transaction is unknown; refresh or recover this swap")
            })?;
        let receipt = wallet.wait_for_receipt(parse(hash)?).await?;
        if !receipt.success {
            return Err(invalid("funding transaction reverted"));
        }
        let sender = options.coordinator.unwrap_or(context.owner);
        let created = matching_creation(&receipt.logs, &context, sender, options.lock_token)?
            .ok_or_else(|| invalid("funding receipt does not contain this HTLC"))?;
        let htlc = EvmHtlc {
            chain_id: context.chain_id,
            address: context.htlc,
            hash_lock: context.hash,
            amount: created.amount,
            token: options.lock_token,
            sender,
            claimer: context.server,
            timelock: context.timelock,
        };
        if htlc.key() != created.key {
            return Err(invalid("funding event HTLC key mismatch"));
        }
        if let Some(coordinator) = options.coordinator {
            let depositor = wallet
                .call(&transaction(
                    wallet,
                    coordinator,
                    Coordinator::depositsCall { key: htlc.key() }.abi_encode(),
                ))
                .await?;
            let bytes = word(&depositor)?.to_be_bytes::<32>();
            if bytes[..12] != [0u8; 12] || Address::from_slice(&bytes[12..]) != context.owner {
                return Err(invalid(
                    "coordinator deposit does not belong to this wallet",
                ));
            }
        }
        Ok(htlc)
    }

    pub async fn refund_evm_with_wallet(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: RefundOptions,
    ) -> Result<EvmReceipt> {
        tokio::time::timeout(options.timeout, async {
            let _execution = self.evm_execution.lock().await;
            let htlc = self.get_funded_evm_htlc(id, wallet, &options).await?;
            if !htlc.is_active(wallet).await? {
                return Err(invalid("HTLC is no longer active"));
            }
            if wallet.block_timestamp().await? < htlc.timelock {
                return Err(invalid("refund timelock has not expired on chain"));
            }
            let (to, data) = if let Some(coordinator) = options.coordinator {
                (
                    coordinator,
                    Coordinator::refundToCall {
                        preimageHash: htlc.hash_lock,
                        amount: htlc.amount,
                        token: htlc.token,
                        claimAddress: htlc.claimer,
                        timelock: U256::from(htlc.timelock),
                    }
                    .abi_encode(),
                )
            } else {
                (
                    htlc.address,
                    Htlc::refundCall {
                        preimageHash: htlc.hash_lock,
                        amount: htlc.amount,
                        token: htlc.token,
                        claimAddress: htlc.claimer,
                        timelock: U256::from(htlc.timelock),
                    }
                    .abi_encode(),
                )
            };
            let tx = transaction(wallet, to, data);
            wallet.call(&tx).await?;
            let hash = wallet.send_transaction(&tx).await?;
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    evm_refund_txid: Some(hash.to_string()),
                    ..Default::default()
                },
            )?;
            let receipt = wallet.wait_for_receipt(hash).await?;
            htlc.verify_refund_receipt(&receipt)?;
            Ok(receipt)
        })
        .await
        .map_err(|_| {
            Error::Timeout("EVM refund timed out; inspect its stored transaction".into())
        })?
    }

    pub async fn get_collab_refund_evm_params(&self, id: &str) -> Result<CollabRefundParams> {
        let mut url = self.base_url.join("/api/swap/")?;
        url.path_segments_mut()
            .map_err(|_| invalid("invalid API URL"))?
            .pop_if_empty()
            .push(id)
            .extend(["collab-refund-evm", "params"]);
        let response = self.api_request(Method::GET, url).send().await?;
        Ok(crate::client::check_status(response).await?.json().await?)
    }

    /// Cooperative refund keeps the output in the locked token, matching the
    /// TypeScript continuation flow. No DEX calls are authorized in this signature.
    pub async fn collab_refund_evm_with_wallet(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: RefundOptions,
    ) -> Result<EvmSubmission> {
        tokio::time::timeout(options.timeout, async {
            let _execution = self.evm_execution.lock().await;
            let htlc = self.get_funded_evm_htlc(id,wallet,&options).await?;
            if !htlc.is_active(wallet).await? { return Err(invalid("HTLC is no longer active")); }
            let coordinator = options.coordinator.ok_or_else(|| invalid("collaborative refunds require coordinator funding"))?;
            let params = self.get_collab_refund_evm_params(id).await?;
            let empty_calls: Vec<crate::aa::abi::Call> = vec![];
            if params.chain_id != htlc.chain_id || parse::<Address>(&params.coordinator_address)? != coordinator
                || parse::<B256>(&params.preimage_hash)? != htlc.hash_lock || decimal(&params.amount)? != htlc.amount
                || parse::<Address>(&params.token)? != htlc.token || parse::<Address>(&params.claim_address)? != htlc.claimer
                || params.timelock != htlc.timelock || parse::<Address>(&params.sweep_token)? != htlc.token
                || !decimal(&params.min_amount_out)?.is_zero() || parse::<B256>(&params.calls_hash)? != keccak256(empty_calls.abi_encode())
                || parse::<Address>(&params.server_signer_address)? != htlc.claimer {
                return Err(invalid("collaborative refund parameters do not match the funded HTLC"));
            }
            verify_version(wallet,coordinator,params.coordinator_version).await?;
            let typed = params.typed_data()?;
            let signature = wallet.sign_typed_data(&typed).await?;
            verify_signature(&signature,typed.digest,wallet.address())?;
            let mut url = self.base_url.join("/api/swap/")?;
            url.path_segments_mut().map_err(|_| invalid("invalid API URL"))?.pop_if_empty().push(id).push("collab-refund-evm");
            let response = self.api_request(Method::POST,url).json(&json!({"v":u8::from(signature.v())+27,"r":format!("0x{:064x}",signature.r()),"s":format!("0x{:064x}",signature.s()),"depositor_address":wallet.address()})).send().await?;
            let submitted: EvmSubmission = crate::client::check_status(response).await?.json().await?;
            if submitted.id != id { return Err(Error::Decode("refund response ID mismatch".into())); }
            let _: B256 = parse(&submitted.tx_hash)?;
            self.update_swap_metadata(id,SwapMetadata { evm_refund_txid: Some(submitted.tx_hash.clone()), ..Default::default() })?;
            Ok(submitted)
        }).await.map_err(|_| Error::Timeout("collaborative EVM refund timed out".into()))?
    }
}

impl CollabRefundParams {
    pub fn typed_data(&self) -> Result<EvmTypedData> {
        typed_data(
            "HTLCCoordinator",
            self.coordinator_version,
            self.chain_id,
            parse(&self.coordinator_address)?,
            "CollabRefund",
            &[
                ("preimageHash", "bytes32"),
                ("amount", "uint256"),
                ("token", "address"),
                ("claimAddress", "address"),
                ("timelock", "uint256"),
                ("caller", "address"),
                ("sweepToken", "address"),
                ("minAmountOut", "uint256"),
                ("callsHash", "bytes32"),
            ],
            json!({"preimageHash":self.preimage_hash,"amount":self.amount,"token":self.token,"claimAddress":self.claim_address,"timelock":self.timelock.to_string(),"caller":self.server_signer_address,"sweepToken":self.sweep_token,"minAmountOut":self.min_amount_out,"callsHash":self.calls_hash}),
        )
    }
}
