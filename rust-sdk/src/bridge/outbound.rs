//! Bridge calls appended to a reviewed coordinator DEX claim.
use super::BridgeSubmission;
use super::cctp;
use super::invalid;
use super::usdt0;
use crate::Client;
use crate::Result;
use crate::aa::abi::IERC20;
use crate::evm::*;
use crate::storage::BridgeOperation;
use crate::types::DexCall;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolEvent;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum OutboundBridge {
    /// `burn.sender` is the pinned adapter; `burn.amount` is the maximum
    /// approved input. The actual DEX output is recovered from MessageSent.
    Cctp {
        burn: cctp::CctpBurn,
        minimum_amount: U256,
    },
    /// `transfer.sender` is the pinned adapter; `amount` caps its approval.
    Usdt0 { transfer: usdt0::OftTransfer },
}
#[derive(Clone, Debug)]
pub struct OutboundClaim {
    /// Fresh DEX calls with output sent to the coordinator, including their
    /// own minimum-output protection. Reviewed by the calling application.
    pub dex_calls: Vec<DexCall>,
    pub bridge: OutboundBridge,
}
#[derive(Clone, Debug)]
pub enum OutboundReceipt {
    Cctp {
        source_txid: B256,
        burn: Box<cctp::CctpBurn>,
    },
    Usdt0(usdt0::MessageQuery),
}
fn dex_call(target: Address, data: Vec<u8>, value: U256) -> DexCall {
    DexCall {
        target: target.to_string(),
        data: format!("0x{}", hex::encode(data)),
        value: value.to_string(),
    }
}
fn approval(token: Address, adapter: Address, amount: U256) -> DexCall {
    dex_call(
        token,
        IERC20::approveCall {
            spender: adapter,
            amount,
        }
        .abi_encode(),
        U256::ZERO,
    )
}
impl OutboundBridge {
    async fn calls(
        &self,
        wallet: &dyn EvmWallet,
        options: &EvmClaimOptions,
    ) -> Result<(Vec<DexCall>, U256)> {
        if !options.min_amount_out.is_zero() {
            return Err(invalid(
                "bridge consumes the coordinator balance; sweep minimum must be zero (DEX calls enforce output protection)",
            ));
        }
        match self {
            Self::Cctp {
                burn,
                minimum_amount,
            } => {
                burn.validate()?;
                let chain = cctp::CctpChain::for_chain(wallet.chain_id())?;
                if chain.chain_id != 42161
                    || burn.source_chain_id != chain.chain_id
                    || burn.source_domain != chain.domain
                    || burn.burn_token != chain.usdc
                    || options.sweep_token != chain.usdc
                    || burn.sender != cctp::ARBITRUM_ADAPTER
                    || burn.source_messenger != cctp::TOKEN_MESSENGER
                    || burn.source_transmitter != cctp::MESSAGE_TRANSMITTER
                    || !burn.destination_caller.is_zero()
                    || burn.min_finality != 1000
                    || *minimum_amount <= burn.max_fee
                    || *minimum_amount > burn.amount
                    || !(burn.hook == cctp::forwarding_hook(None)
                        || (burn.destination_domain == cctp::SOLANA_DOMAIN
                            && burn.hook.len() == 65
                            && burn.hook[..33] == cctp::forwarding_hook(Some([0; 32]))[..33]))
                {
                    return Err(invalid("invalid CCTP coordinator bridge route"));
                }
                Ok((
                    vec![
                        approval(chain.usdc, burn.sender, burn.amount),
                        dex_call(
                            burn.sender,
                            cctp::bridgeBalanceCall {
                                destinationDomain: burn.destination_domain,
                                mintRecipient: burn.mint_recipient,
                                maxFee: burn.max_fee,
                                hookData: burn.hook.clone(),
                            }
                            .abi_encode(),
                            U256::ZERO,
                        ),
                        approval(chain.usdc, burn.sender, U256::ZERO),
                    ],
                    U256::ZERO,
                ))
            }
            Self::Usdt0 { transfer } => {
                transfer.params()?;
                let token = usdt0::CHAINS
                    .iter()
                    .find(|chain| chain.name == "Arbitrum")
                    .expect("registry contains Arbitrum")
                    .token;
                if wallet.chain_id() != 42161
                    || transfer.chain_id != 42161
                    || transfer.source_eid != 30110
                    || transfer.oft != usdt0::ARBITRUM_OFT
                    || transfer.sender != usdt0::ARBITRUM_ADAPTER
                    || transfer.token != token
                    || options.sweep_token != token
                {
                    return Err(invalid("invalid USDT0 coordinator bridge route"));
                }
                // quoteSend is independent of msg.sender. Query from the caller's
                // wallet; the actual send is executed by the pinned adapter.
                let mut quote = transfer.clone();
                quote.sender = wallet.address();
                let fee = quote.quote(wallet).await?;
                Ok((
                    vec![
                        approval(token, transfer.sender, transfer.amount),
                        dex_call(
                            transfer.sender,
                            usdt0::bridgeBalanceCall {
                                dstEid: transfer.destination_eid,
                                to: transfer.recipient,
                            }
                            .abi_encode(),
                            fee,
                        ),
                        approval(token, transfer.sender, U256::ZERO),
                    ],
                    transfer.max_native_fee,
                ))
            }
        }
    }
    fn verify(&self, receipt: &EvmReceipt, coordinator: Address) -> Result<OutboundReceipt> {
        if !receipt.success {
            return Err(invalid("outbound bridge claim reverted"));
        }
        match self {
            Self::Cctp {
                burn,
                minimum_amount,
            } => {
                let mut matching = Vec::new();
                for log in receipt
                    .logs
                    .iter()
                    .filter(|log| log.address == burn.source_transmitter)
                {
                    let Ok(event) = cctp::MessageSent::decode_raw_log_validate(
                        log.topics.iter().copied(),
                        &log.data,
                    ) else {
                        continue;
                    };
                    let Ok(message) = cctp::CctpMessage::decode(&event.message) else {
                        continue;
                    };
                    if message.amount < *minimum_amount || message.amount > burn.amount {
                        continue;
                    }
                    let mut actual = burn.clone();
                    actual.amount = message.amount;
                    if actual.validate_message(&event.message).is_ok() {
                        matching.push(actual);
                    }
                }
                if matching.len() != 1 {
                    return Err(invalid("claim receipt has no unique approved CCTP burn"));
                }
                Ok(OutboundReceipt::Cctp {
                    source_txid: receipt.transaction_hash,
                    burn: Box::new(matching.remove(0)),
                })
            }
            Self::Usdt0 { transfer } => {
                // The adapter event includes the recipient, unlike OFTSent.
                let bridged = receipt
                    .logs
                    .iter()
                    .filter(|log| log.address == transfer.sender)
                    .filter_map(|log| {
                        OftBridgeInitiated::decode_raw_log_validate(
                            log.topics.iter().copied(),
                            &log.data,
                        )
                        .ok()
                    })
                    .find(|event| {
                        event.dstEid == transfer.destination_eid
                            && event.to == transfer.recipient
                            && event.caller == coordinator
                            && event.amount <= transfer.amount
                            && event.amount >= transfer.minimum_received
                            && event.nativeFee <= transfer.max_native_fee
                    })
                    .ok_or_else(|| {
                        invalid("claim receipt does not match the OFT recipient and limits")
                    })?;
                let mut expected = transfer.clone();
                expected.amount = bridged.amount;
                let guid = super::execution::validate_oft_receipt(receipt, &expected)?;
                Ok(OutboundReceipt::Usdt0(usdt0::MessageQuery {
                    source_txid: receipt.transaction_hash,
                    source_eid: transfer.source_eid,
                    destination_eid: transfer.destination_eid,
                    guid,
                }))
            }
        }
    }
}
alloy::sol! {
    // Name must match the deployed adapter event for its signature hash.
    interface OftAdapterEvents {
        event BridgeInitiated(uint32 indexed dstEid, bytes32 to, uint256 amount, uint256 nativeFee, address indexed caller);
    }
}
use OftAdapterEvents::BridgeInitiated as OftBridgeInitiated;

impl Client {
    /// Claim and bridge the DEX output. A restart reads the stored claim receipt
    /// and reconstructs the CCTP burn or LayerZero GUID without revealing the
    /// preimage again. Mainnet adapter deployments currently exist on Arbitrum.
    pub async fn claim_evm_to_bridge(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: EvmClaimOptions,
        request: OutboundClaim,
        mode: BridgeSubmission<'_>,
    ) -> Result<OutboundReceipt> {
        tokio::time::timeout(options.timeout,async {
            let intent=json!({"bridge":request.bridge,"coordinator":options.coordinator,"htlc":options.htlc,"destination":options.destination,"sweep_token":options.sweep_token});
            {
                let _guard=self.evm_execution.lock().await;
                if let Some(existing)=self.bridge_operation(id,"outbound_claim")? {
                    if existing.intent!=intent {return Err(invalid("outbound bridge differs from the persisted claim"));}
                } else {
                    // Validate before reserving the route or revealing any secret.
                    request.bridge.calls(wallet,&options).await?;
                    self.save_bridge_operation(id,"outbound_claim",BridgeOperation {intent: intent.clone(),transaction_hash:None,user_operation_hash:None,complete:false})?;
                }
            }
            let stored=self.get_stored_swap(id)?.ok_or_else(||invalid("swap record missing"))?;
            let mut hash=stored.metadata.evm_claim_txid.map(|value|crate::evm::parse::<B256>(&value)).transpose()?;
            if hash.is_none() && stored.metadata.evm_claim_userop_hash.is_some() {
                let BridgeSubmission::UserOperation(config)=&mode else {return Err(invalid("resume the sponsored claim with its bundler configuration"));};
                let bundler=crate::aa::bundler::BundlerClient::new(config.aa.bundler_url.clone(),config.aa.bundler_casing)?;
                hash=self.resume_evm_claim_userop(id,&bundler).await?;
                if hash.is_none() {return Err(invalid("outbound claim UserOperation is still pending"));}
            }
            let receipt=if let Some(hash)=hash {
                let receipt=wallet.wait_for_receipt(hash).await?;
                if receipt.transaction_hash!=hash {return Err(invalid("outbound receipt hash mismatch"));}
                receipt
            } else {
                let (bridge_calls,budget)=request.bridge.calls(wallet,&options).await?;
                if request.dex_calls.iter().any(|call|call.value!="0") {return Err(invalid("DEX native value must be zero; only bridge fees are approved"));}
                let mut calls=request.dex_calls; calls.extend(bridge_calls);
                match mode {
                    BridgeSubmission::Wallet=>self.claim_evm_with_native_fee(id,wallet,options.clone(),calls,budget).await?,
                    BridgeSubmission::UserOperation(config)=>{
                        let receipt=self.claim_evm_userop_with_native_fee(id,wallet,options.clone(),calls,config,budget).await?;
                        let hash=receipt.transaction_hash.ok_or_else(||invalid("outbound claim UserOperation pending"))?;
                        wallet.wait_for_receipt(hash).await?
                    }
                }
            };
            self.verify_evm_claim_receipt(id,wallet,&options,&receipt).await?;
            let result=request.bridge.verify(&receipt,options.coordinator)?;
            let mut state=self.bridge_operation(id,"outbound_claim")?.ok_or_else(||invalid("outbound claim intent missing"))?;
            state.transaction_hash=Some(receipt.transaction_hash.to_string()); state.complete=true;
            self.save_bridge_operation(id,"outbound_claim",state)?;
            Ok(result)
        }).await.map_err(|_|crate::Error::Timeout("outbound bridge claim deadline exceeded; inspect persisted claim".into()))?
    }
}
