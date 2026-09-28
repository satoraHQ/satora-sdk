//! Atomic CCTP mint + Permit2 funding. Also accepts already-forwarded mints.
use super::BridgeSubmission;
use super::CctpMint;
use super::execution::Journal;
use super::invalid;
use crate::Client;
use crate::Result;
use crate::SwapMetadata;
use crate::aa::UserOpConfig;
use crate::aa::abi::Call;
use crate::aa::abi::IERC20;
use crate::aa::orchestrate::FundSwapInputs;
use crate::aa::orchestrate::funding_calls;
use crate::control::PollOptions;
use crate::evm::funding::matching_creation;
use crate::evm::funding::transaction;
use crate::evm::funding::word;
use crate::evm::htlc::EvmHtlc;
use crate::evm::*;
use alloy::sol_types::SolCall;
use serde_json::json;

#[derive(Clone, Debug)]
pub struct CctpFunding<'a> {
    pub mint: CctpMint<'a>,
    pub limits: FundingOptions,
    pub account: &'a UserOpConfig,
}
impl Client {
    /// Mint USDC and fund the swap in one sponsored transaction. Source burning
    /// is a separate resumable operation (`burn_cctp`); fetch its attestation with
    /// `BridgeApi::wait_cctp_attestation`. No USDC remains stranded if funding reverts.
    /// Operation key `cctp_fund` is resumable without submitting another batch.
    pub async fn fund_swap_from_cctp(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        request: CctpFunding<'_>,
        options: &PollOptions,
    ) -> Result<EvmReceipt> {
        options.run(async {
            let _guard=self.evm_execution.lock().await;
            let CctpFunding {mint,limits,account}=request;
            let CctpMint {burn,destination,attestation}=mint;
            destination.validate(burn,wallet)?;
            let receive=attestation.receive_calldata(burn)?;
            let message:Bytes=crate::evm::parse(&attestation.message)?;
            let decoded=burn.validate_message(&message)?;
            if self.cctp_recovery_recorded(burn.source_domain,decoded.nonce)? {return Err(invalid("this burn has a recovery entry; its USDC was or is being moved to the recovery recipient"));}
            if burn.mint_recipient!=wallet.address().into_word() {return Err(invalid("CCTP mint recipient must be the funding account"));}
            let stored=self.get_stored_swap(id)?.ok_or_else(||invalid("swap record missing"))?;
            let context=EvmContext::from_response(&stored.response)?;
            context.validate_wallet(wallet)?;
            if !context.is_source || context.source_token!=destination.usdc {return Err(invalid("CCTP funding requires a USDC-source swap on the destination chain"));}
            let intent=json!({"burn":burn,"destination":destination,"nonce":decoded.nonce,"coordinator":limits.coordinator,"htlc":limits.htlc,"lock_token":limits.lock_token,"max_source_amount":limits.max_source_amount,"allowed_call_targets":limits.allowed_call_targets});
            let mode=BridgeSubmission::UserOperation(account);
            let journal=Journal::Swap {id,key:"cctp_fund"};
            let receipt=match self.bridge_receipt(journal,&intent,wallet,&mode).await? {
                Some(receipt)=>receipt,
                None=>{
                    if stored.metadata.evm_fund_txid.is_some() || stored.metadata.evm_fund_userop_hash.is_some() {
                        return Err(invalid("swap funding already submitted"));
                    }
                    if stored.metadata.relay_signature_outstanding() {return Err(invalid(crate::evm::funding::UNRESOLVED_RELAY_FUNDING));}
                    if stored.metadata.evm_fund_started == Some(true) {return Err(invalid(crate::evm::funding::UNRESOLVED_FUNDING));}
                    crate::evm::verify_derived_hash_lock(self,&stored,&context)?;
                    if wallet.block_timestamp().await? >= context.timelock {return Err(invalid("swap timelock has expired on chain"));}
                    let used=word(&wallet.call(&transaction(wallet,destination.transmitter,super::cctp::usedNoncesCall {nonce:decoded.nonce}.abi_encode())).await?)?;
                    if used>U256::ONE {return Err(invalid("invalid CCTP nonce state"));}
                    let funding=self.get_permit2_funding_data(id).await?;
                    let dex=funding.validate(&context,&limits)?;
                    let amount=crate::evm::decimal(&funding.source_amount)?;
                    if amount>decoded.minted_amount() {return Err(invalid("funding exceeds the attested net mint amount"));}
                    let balance=word(&wallet.call(&transaction(wallet,destination.usdc,IERC20::balanceOfCall {account:wallet.address()}.abi_encode())).await?)?;
                    if used==U256::ONE && balance<amount {return Err(invalid("already minted USDC is unavailable for funding"));}
                    let key=self.signer.as_ref().ok_or_else(||invalid("signer missing"))?.evm_key_for_address(stored.key_index,&wallet.address().to_string())?;
                    let mut calls=Vec::new();
                    if used.is_zero() {calls.push(Call {target:destination.transmitter,value:U256::ZERO,callData:receive});}
                    calls.extend(funding_calls(&FundSwapInputs {
                        coordinator_address:limits.coordinator,source_token_address:destination.usdc,source_amount:amount,
                        lock_token_address:limits.lock_token,preimage_hash:context.hash,claim_address:context.server,timelock:context.timelock,
                        calls:dex,calls_hash:crate::evm::parse(&funding.calls_hash)?,entry_point:account.entry_point,delegation_target:account.delegation_target,
                        secret_key:key.secret_key,eoa_address:context.owner,chain_id:context.chain_id,gas_overrides:account.aa.gas_overrides,
                    })?);
                    self.submit_bridge(journal,intent,wallet,&mode,calls).await?
                }
            };
            // Persist the funding identity even if event validation fails, preventing
            // another execution path from submitting a duplicate funding operation.
            self.update_swap_metadata(id,SwapMetadata {evm_fund_txid:Some(receipt.transaction_hash.to_string()),evm_coordinator_address:Some(limits.coordinator.to_string()),..Default::default()})?;
            let created=matching_creation(&receipt.logs,&context,limits.coordinator,limits.lock_token)?
                .ok_or_else(||invalid("atomic CCTP funding has no matching HTLC event"))?;
            let htlc=EvmHtlc {chain_id:context.chain_id,address:context.htlc,hash_lock:context.hash,amount:created.amount,token:limits.lock_token,sender:limits.coordinator,claimer:context.server,timelock:context.timelock};
            if created.amount.is_zero() || created.key!=htlc.key() {return Err(invalid("invalid funded HTLC identity"));}
            self.complete_bridge(journal)?;
            Ok(receipt)
        }).await
    }
}
