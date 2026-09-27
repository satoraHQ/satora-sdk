//! Receipt-verified refund continuation and stranded ERC-20 recovery.
use super::funding::transaction;
use super::funding::word;
use super::*;
use crate::Client;
use crate::SwapMetadata;
use crate::aa::abi::IERC20;
use crate::aa::bundler::BundlerClient;
use crate::types::EvmAsset;
use crate::types::GetSwapResponse;
use crate::types::QuoteAmount;
use crate::types::SwapCreation;
use crate::types::SwapOptions;
use crate::types::TokenId;
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolEvent;
use std::time::Duration;

sol! {
    interface RecoveryToken { function transfer(address to, uint256 amount) external returns (bool); }
    event Transfer(address indexed from, address indexed to, uint256 value);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefundedEvmBalance {
    pub chain_id: u64,
    pub token: Address,
    pub amount: U256,
    pub balance: U256,
}

#[derive(Clone, Debug)]
pub struct DepositRecovery {
    pub token: Address,
    pub destination: Address,
    pub amount: U256,
    pub timeout: Duration,
}

async fn balance(wallet: &dyn EvmWallet, token: Address) -> Result<U256> {
    word(
        &wallet
            .call(&transaction(
                wallet,
                token,
                IERC20::balanceOfCall {
                    account: wallet.address(),
                }
                .abi_encode(),
            ))
            .await?,
    )
}

fn verify_transfer(
    receipt: &EvmReceipt,
    token: Address,
    from: Option<Address>,
    to: Address,
    amount: U256,
) -> Result<()> {
    if receipt.success
        && receipt
            .logs
            .iter()
            .filter(|log| log.address == token)
            .any(|log| {
                Transfer::decode_raw_log_validate(log.topics.iter().copied(), &log.data).is_ok_and(
                    |event| {
                        from.is_none_or(|from| from == event.from)
                            && event.to == to
                            && event.value == amount
                    },
                )
            })
    {
        Ok(())
    } else {
        Err(invalid(
            "receipt does not contain the expected token transfer",
        ))
    }
}

/// The recovery hash still awaiting verification. Once verified it is copied to
/// `evm_recovery_settled`, because merge cannot clear `evm_recovery_txid`.
fn pending_recovery(metadata: &SwapMetadata) -> Option<&String> {
    metadata
        .evm_recovery_txid
        .as_ref()
        .filter(|hash| metadata.evm_recovery_settled.as_ref() != Some(*hash))
}

impl Client {
    /// Attach the original funding hash after a lost broadcast response. This
    /// records identity only; call `get_funded_evm_htlc` to verify its receipt.
    pub async fn attach_evm_funding_transaction(&self, id: &str, hash: B256) -> Result<()> {
        let _guard = self.evm_execution.lock().await;
        let metadata = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?
            .metadata;
        if hash.is_zero()
            || metadata.evm_fund_started != Some(true)
            || metadata.evm_fund_txid.is_some()
        {
            return Err(invalid("no unresolved funding submission to reconcile"));
        }
        self.update_swap_metadata(
            id,
            SwapMetadata {
                evm_fund_txid: Some(hash.to_string()),
                ..Default::default()
            },
        )
    }
    /// Attach the original refund hash after a lost response, then call the
    /// refund method again to verify the receipt without rebroadcasting.
    pub async fn attach_evm_refund_transaction(&self, id: &str, hash: B256) -> Result<()> {
        let _guard = self.evm_execution.lock().await;
        let metadata = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?
            .metadata;
        if hash.is_zero()
            || metadata.evm_refund_started != Some(true)
            || metadata.evm_refund_txid.is_some()
        {
            return Err(invalid("no unresolved refund submission to reconcile"));
        }
        self.update_swap_metadata(
            id,
            SwapMetadata {
                evm_refund_txid: Some(hash.to_string()),
                ..Default::default()
            },
        )
    }

    /// Reconcile the original stranded-token transfer after a lost response.
    /// Retry `recover_evm_deposit_with_wallet` with the same approved limits to
    /// verify its receipt; attaching a hash alone does not establish settlement.
    pub async fn attach_evm_recovery_transaction(&self, id: &str, hash: B256) -> Result<()> {
        let _guard = self.evm_execution.lock().await;
        let metadata = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?
            .metadata;
        if hash.is_zero()
            || metadata.evm_recovery_started != Some(true)
            || pending_recovery(&metadata).is_some()
        {
            return Err(invalid("no unresolved recovery submission to reconcile"));
        }
        self.update_swap_metadata(
            id,
            SwapMetadata {
                evm_recovery_txid: Some(hash.to_string()),
                ..Default::default()
            },
        )
    }

    /// Resume receipt discovery after a process restart, without submitting another UserOperation.
    /// `None` means the bundler still reports the persisted operation as pending.
    pub async fn resume_gasless_funding(
        &self,
        id: &str,
        bundler: &BundlerClient,
        wallet: &dyn EvmWallet,
    ) -> Result<Option<B256>> {
        tokio::time::timeout(
            RESUME_TIMEOUT,
            self.resume_gasless_funding_inner(id, bundler, wallet),
        )
        .await
        .map_err(|_| Error::Timeout("funding operation lookup timed out".into()))?
    }

    async fn resume_gasless_funding_inner(
        &self,
        id: &str,
        bundler: &BundlerClient,
        wallet: &dyn EvmWallet,
    ) -> Result<Option<B256>> {
        let _execution = self.evm_execution.lock().await;
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        if let Some(hash) = stored.metadata.evm_fund_txid {
            return Ok(Some(parse(&hash)?));
        }
        let hash = stored
            .metadata
            .evm_fund_userop_hash
            .ok_or_else(|| invalid("no submitted funding operation"))?;
        let op = parse(&hash)?;
        let Some(receipt) = bundler.get_user_operation_receipt(op).await? else {
            return Ok(None);
        };
        if !receipt.success {
            return Err(invalid("funding UserOperation reverted"));
        }
        let hash = receipt.receipt.transaction_hash;
        // fund_swap_gasless refuses any EntryPoint other than the pinned one.
        crate::aa::orchestrate::verify_user_operation_receipt(
            &wallet.wait_for_receipt(hash).await?,
            crate::aa::userop::ENTRY_POINT_V07,
            op,
            hash,
        )?;
        self.update_swap_metadata(
            id,
            SwapMetadata {
                evm_fund_txid: Some(hash.to_string()),
                ..Default::default()
            },
        )?;
        Ok(Some(hash))
    }

    /// Require a terminal refund snapshot, a verified refund event and token transfer,
    /// and an available balance. Quotes alone are never evidence of refunded funds.
    pub async fn get_refunded_evm_swap_continuation(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: &RefundOptions,
    ) -> Result<RefundedEvmBalance> {
        tokio::time::timeout(options.timeout, async {
            let stored = self
                .get_stored_swap(id)?
                .ok_or_else(|| invalid("swap record missing"))?;
            if !matches!(
                stored.response,
                GetSwapResponse::EvmToArkade(_) | GetSwapResponse::EvmToBitcoin(_)
            ) {
                return Err(invalid("continuation requires EVM to Bitcoin or Arkade"));
            }
            let status = serde_json::to_value(stored.response.status())
                .map_err(|e| Error::Decode(e.to_string()))?;
            if !matches!(
                status.as_str(),
                Some("clientrefunded" | "clientrefundedserverrefunded")
            ) {
                return Err(invalid("swap is not safely refunded; refresh its status"));
            }
            let htlc = self
                .read_funded_evm_htlc(id, wallet, options, false)
                .await?;
            if htlc.token.is_zero() {
                return Err(invalid(
                    "continuing a native refund is not supported: the replacement is funded through Permit2",
                ));
            }
            let hash = stored
                .metadata
                .evm_refund_txid
                .ok_or_else(|| invalid("refund transaction missing"))?;
            let receipt = wallet.wait_for_receipt(parse(&hash)?).await?;
            htlc.verify_refund_receipt(&receipt)?;
            verify_transfer(&receipt, htlc.token, None, wallet.address(), htlc.amount)?;
            let available = balance(wallet, htlc.token).await?;
            if available < htlc.amount {
                return Err(invalid("refunded tokens are no longer available"));
            }
            Ok(RefundedEvmBalance {
                chain_id: htlc.chain_id,
                token: htlc.token,
                amount: htlc.amount,
                balance: available,
            })
        })
        .await
        .map_err(|_| Error::Timeout("refund continuation lookup timed out".into()))?
    }

    /// Create or recover a replacement swap ID before funding. The replacement
    /// uses the refunded token and amount, retaining the original output rail.
    /// The caller may fund this ID with either an external wallet or a gasless account.
    pub async fn prepare_refunded_evm_swap_continuation(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        refund: &RefundOptions,
        destination: &str,
    ) -> Result<String> {
        tokio::time::timeout(
            refund.timeout,
            self.prepare_refunded_evm_swap_continuation_inner(id, wallet, refund, destination),
        )
        .await
        .map_err(|_| Error::Timeout("refund continuation timed out".into()))?
    }

    async fn prepare_refunded_evm_swap_continuation_inner(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        refund: &RefundOptions,
        destination: &str,
    ) -> Result<String> {
        let _execution = self.evm_execution.lock().await;
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        if let Some(replacement) = stored.metadata.continuation_swap_id {
            let next = self
                .get_stored_swap(&replacement)?
                .ok_or_else(|| invalid("replacement swap record missing"))?;
            EvmContext::from_response(&next.response)?.validate_wallet(wallet)?;
            if next.metadata.target_address.as_deref() != Some(destination) {
                return Err(invalid(
                    "replacement destination differs from the persisted continuation",
                ));
            }
            return Ok(replacement);
        }
        let funds = self
            .get_refunded_evm_swap_continuation(id, wallet, refund)
            .await?;
        let amount = u128::try_from(funds.amount)
            .map_err(|_| invalid("refund amount exceeds the creation API range"))?;
        let source = EvmAsset {
            chain_id: funds.chain_id,
            token: TokenId::Other(funds.token.to_string()),
        };
        let request = match stored.response {
            GetSwapResponse::EvmToArkade(_) => SwapCreation::EvmToArkade {
                source,
                amount: QuoteAmount::Source(amount),
                target_address: destination.into(),
            },
            GetSwapResponse::EvmToBitcoin(_) => SwapCreation::EvmToBitcoin {
                source,
                amount: QuoteAmount::Source(amount),
                target_address: Some(destination.into()),
            },
            _ => return Err(invalid("unsupported continuation direction")),
        };
        let swap = self
            .create_swap_request(
                request,
                SwapOptions {
                    evm_address: Some(wallet.address().to_string()),
                    ..Default::default()
                },
            )
            .await?;
        self.update_swap_metadata(
            id,
            SwapMetadata {
                continuation_swap_id: Some(swap.id.clone()),
                ..Default::default()
            },
        )?;
        Ok(swap.id)
    }

    /// Create/resume a replacement and fund it through the supplied external wallet.
    /// Use `prepare_refunded_evm_swap_continuation` with `fund_swap_gasless` for AA wallets.
    pub async fn continue_refunded_evm_swap(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        refund: &RefundOptions,
        destination: &str,
        mut funding: FundingOptions,
    ) -> Result<(String, EvmReceipt)> {
        let replacement = self
            .prepare_refunded_evm_swap_continuation(id, wallet, refund, destination)
            .await?;
        let stored = self
            .get_stored_swap(&replacement)?
            .ok_or_else(|| invalid("replacement record missing"))?;
        if let Some(hash) = stored.metadata.evm_fund_txid {
            let receipt = wallet.wait_for_receipt(parse(&hash)?).await?;
            self.get_funded_evm_htlc(
                &replacement,
                wallet,
                &RefundOptions {
                    htlc: funding.htlc,
                    lock_token: funding.lock_token,
                    coordinator: Some(funding.coordinator),
                    timeout: funding.timeout,
                },
            )
            .await?;
            return Ok((replacement, receipt));
        }
        let funds = self
            .get_refunded_evm_swap_continuation(id, wallet, refund)
            .await?;
        funding.max_source_amount = funding.max_source_amount.min(funds.amount);
        let receipt = self
            .fund_swap_with_wallet(&replacement, wallet, funding)
            .await?;
        Ok((replacement, receipt))
    }

    /// Recover tokens held at the swap depositor address, including accidental
    /// deposits. An explicit token, destination and maximum amount are required.
    /// A persisted submission is verified on retry, never sent a second time.
    /// Once settled, identical parameters return the settled receipt and
    /// different parameters start a new transfer.
    pub async fn recover_evm_deposit_with_wallet(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: DepositRecovery,
    ) -> Result<EvmReceipt> {
        tokio::time::timeout(options.timeout, async {
            let _execution = self.evm_execution.lock().await;
            let stored = self
                .get_stored_swap(id)?
                .ok_or_else(|| invalid("swap record missing"))?;
            let context = EvmContext::from_response(&stored.response)?;
            context.validate_wallet(wallet)?;
            if !context.is_source
                || options.destination.is_zero()
                || options.destination == wallet.address()
                || options.token.is_zero()
                || options.amount.is_zero()
            {
                return Err(invalid("invalid deposit recovery parameters"));
            }
            let metadata = stored.metadata;
            let intent = format!(
                "{}:{}:{}",
                options.token, options.destination, options.amount
            );
            let hash = if let Some(hash) = pending_recovery(&metadata) {
                parse(hash)?
            } else {
                if metadata.evm_recovery_started == Some(true) {
                    return Err(invalid(
                        "recovery submission unresolved; attach its transaction, or release evm_recovery_started once nothing was sent",
                    ));
                }
                // Only a repeat of the settled request needs its receipt, so a node
                // that no longer serves an old receipt cannot block a different recovery.
                if let Some(settled) = &metadata.evm_recovery_settled
                    && metadata.evm_recovery_settled_intent.as_ref() == Some(&intent)
                {
                    let prior = parse(settled)?;
                    let receipt = wallet.wait_for_receipt(prior).await?;
                    if receipt.transaction_hash != prior {
                        return Err(invalid("recovery receipt hash mismatch"));
                    }
                    verify_transfer(
                        &receipt,
                        options.token,
                        Some(wallet.address()),
                        options.destination,
                        options.amount,
                    )?;
                    return Ok(receipt);
                }
                if balance(wallet, options.token).await? < options.amount {
                    return Err(invalid("insufficient token balance for recovery"));
                }
                let tx = transaction(
                    wallet,
                    options.token,
                    RecoveryToken::transferCall {
                        to: options.destination,
                        amount: options.amount,
                    }
                    .abi_encode(),
                );
                let result = wallet.call(&tx).await?;
                if !result.is_empty() && word(&result)? != U256::ONE {
                    return Err(invalid("token transfer simulation returned false"));
                }
                self.update_swap_metadata(
                    id,
                    SwapMetadata {
                        evm_recovery_started: Some(true),
                        ..Default::default()
                    },
                )?;
                let hash = wallet.send_transaction(&tx).await?;
                self.update_swap_metadata(
                    id,
                    SwapMetadata {
                        evm_recovery_txid: Some(hash.to_string()),
                        ..Default::default()
                    },
                )?;
                hash
            };
            let receipt = wallet.wait_for_receipt(hash).await?;
            if receipt.transaction_hash != hash {
                return Err(invalid("recovery receipt hash mismatch"));
            }
            verify_transfer(
                &receipt,
                options.token,
                Some(wallet.address()),
                options.destination,
                options.amount,
            )?;
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    evm_recovery_started: Some(false),
                    evm_recovery_settled: Some(hash.to_string()),
                    evm_recovery_settled_intent: Some(intent),
                    ..Default::default()
                },
            )?;
            Ok(receipt)
        })
        .await
        .map_err(|_| {
            Error::Timeout("deposit recovery timed out; inspect its stored transaction".into())
        })?
    }
}
