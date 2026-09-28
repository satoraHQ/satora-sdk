//! Recovering CCTP-inbound USDC from the source burn's transaction hash, with or
//! without a local swap record (TypeScript `recoverCctpInbound`).
use super::BridgeApi;
use super::BridgeSubmission;
use super::CctpDestination;
use super::cctp;
use super::cctp::CctpAttestation;
use super::cctp::CctpBurn;
use super::cctp::CctpSource;
use super::execution::Journal;
use super::execution::Token;
use super::execution::Transfer;
use super::execution::received_message;
use super::execution::token_balance;
use super::invalid;
use crate::Client;
use crate::Error;
use crate::Result;
use crate::aa::abi::Call;
use crate::control::PollOptions;
use crate::evm::funding::transaction;
use crate::evm::funding::word;
use crate::evm::*;
use crate::signer::EvmKey;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolEvent;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;

/// Seed keys scanned for an orphaned mint recipient, as far as the server's
/// swap recovery scans.
const RECOVERY_SCAN: u32 = 1_000;

/// How much a recovery moves to its recipient.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CctpSweep {
    /// The burn's net minted amount. Other USDC on the account stays put, and a
    /// repeated recovery never moves more.
    #[default]
    Minted,
    /// The account's whole USDC balance, as TypeScript does. On an account that
    /// several swaps share (the wallet key, or a deposit address) this also moves
    /// USDC reserved for another swap's funding.
    FullBalance,
}

pub enum CctpRecoveryAccount<'a> {
    /// A key from the SDK seed that controls the mint recipient: a stored swap's
    /// key, the wallet key (`m/44'/60'/0'/0/0`), the `key_index` hint, or a scan of
    /// the first 1000 per-swap and legacy keys. `destination_rpc` reads and submits.
    Local {
        destination_rpc: url::Url,
        key_index: Option<u32>,
    },
    /// An external wallet that is the mint recipient. Wallet submission only.
    External(&'a dyn EvmWallet),
}

pub struct CctpInboundRecovery<'a> {
    pub source: CctpSource,
    pub source_rpc: url::Url,
    pub source_txid: B256,
    pub destination: CctpDestination,
    pub recipient: Address,
    pub account: CctpRecoveryAccount<'a>,
    pub submission: BridgeSubmission<'a>,
    pub sweep: CctpSweep,
    /// Skips polling IRIS. It is checked against the burn like a fetched one.
    pub attestation: Option<CctpAttestation>,
}

#[derive(Clone, Debug)]
pub struct CctpInboundRecovered {
    pub burn: CctpBurn,
    pub nonce: B256,
    pub account: Address,
    /// The stored swap that owns the account, when exactly one does.
    pub swap_id: Option<String>,
    /// Journal key of this burn; wallet submissions use its `:mint` and `:sweep` entries.
    pub journal_key: String,
    /// Set when this recovery minted; `None` when the nonce was already consumed.
    pub mint_transaction: Option<B256>,
    pub sweep: EvmReceipt,
    pub recovered_amount: U256,
}

/// The one burn in the source receipt that the account can recover.
pub(super) struct Selected {
    pub(super) burn: CctpBurn,
    pub(super) account: Address,
    pub(super) key: Option<EvmKey>,
}

fn recovery_key(source_domain: u32, nonce: B256) -> String {
    format!("cctp_recover:{source_domain}:{nonce}")
}

impl CctpInboundRecovery<'_> {
    fn check(&self) -> Result<()> {
        if matches!(
            (&self.account, &self.submission),
            (
                CctpRecoveryAccount::External(_),
                BridgeSubmission::UserOperation(_)
            )
        ) {
            return Err(invalid(
                "sponsored recovery signs with a seed key; external wallets use Wallet submission",
            ));
        }
        if self.recipient.is_zero()
            || self.source.chain_id == self.destination.chain_id
            || self.source.domain == self.destination.domain
            || self.source.messenger.is_zero()
            || self.source.transmitter.is_zero()
            || self.source.usdc.is_zero()
            || self.destination.transmitter.is_zero()
            || self.destination.usdc.is_zero()
        {
            return Err(invalid("invalid CCTP recovery request"));
        }
        Ok(())
    }
}

async fn source_receipt(rpc: &url::Url, chain_id: u64, hash: B256) -> Result<EvmReceipt> {
    use alloy::providers::Provider;
    let transport = |e: alloy::transports::TransportError| Error::Transport(e.to_string());
    let provider = crate::evm::http_provider(rpc.clone());
    if provider.get_chain_id().await.map_err(transport)? != chain_id {
        return Err(invalid("source RPC is on another chain"));
    }
    let receipt = provider
        .get_transaction_receipt(hash)
        .await
        .map_err(transport)?
        .ok_or_else(|| invalid("source transaction not found"))?;
    let receipt = crate::evm::evm_receipt(&receipt)?;
    if receipt.transaction_hash != hash {
        return Err(invalid("source receipt hash mismatch"));
    }
    Ok(receipt)
}

fn transferred(
    receipt: &EvmReceipt,
    token: Address,
    from: Address,
    to: Address,
    amount: Option<U256>,
) -> Result<U256> {
    receipt
        .logs
        .iter()
        .filter(|log| log.address == token)
        .filter_map(|log| {
            Transfer::decode_raw_log_validate(log.topics.iter().copied(), &log.data).ok()
        })
        .find(|event| {
            event.from == from
                && event.to == to
                && !event.value.is_zero()
                && amount.is_none_or(|amount| event.value == amount)
        })
        .map(|event| event.value)
        .ok_or_else(|| invalid("receipt does not contain the recovery transfer"))
}

impl Client {
    /// Mint a stalled CCTP-inbound burn if its nonce is unused, then move USDC
    /// from the minting account to `recipient` (TypeScript `recoverCctpInbound`).
    /// Needs no swap record. The burn terms come from the source receipt, never
    /// from IRIS alone. Progress is journaled per burn: a repeat returns the
    /// recorded result, and a lost submission blocks until
    /// `attach_cctp_recovery_transaction` or `release_cctp_recovery`. A repeat in
    /// the other submission mode is refused once a transfer, or an unresolved
    /// wallet mint, is journaled.
    pub async fn recover_cctp_inbound(
        &self,
        api: &BridgeApi,
        recovery: CctpInboundRecovery<'_>,
        options: &PollOptions,
    ) -> Result<CctpInboundRecovered> {
        options
            .run(async {
                recovery.check()?;
                let receipt = source_receipt(
                    &recovery.source_rpc,
                    recovery.source.chain_id,
                    recovery.source_txid,
                )
                .await?;
                let selected = self.select_recovery(&recovery, &receipt)?;
                match &recovery.account {
                    CctpRecoveryAccount::External(wallet) => {
                        self.execute_recovery(api, &recovery, selected, *wallet, options)
                            .await
                    }
                    CctpRecoveryAccount::Local {
                        destination_rpc, ..
                    } => {
                        let key = selected
                            .key
                            .clone()
                            .ok_or_else(|| Error::InvalidSigner("no seed key selected".into()))?;
                        let wallet = RpcWallet::connect(
                            &key,
                            recovery.destination.chain_id,
                            destination_rpc.clone(),
                        )
                        .await?;
                        self.execute_recovery(api, &recovery, selected, &wallet, options)
                            .await
                    }
                }
            })
            .await
    }

    /// Record the transaction of a recovery submission whose response was lost,
    /// then repeat `recover_cctp_inbound` to verify it.
    pub async fn attach_cctp_recovery_transaction(
        &self,
        journal_key: &str,
        hash: B256,
    ) -> Result<()> {
        let _guard = self.evm_execution.lock().await;
        if hash.is_zero() {
            return Err(invalid("invalid recovery transaction hash"));
        }
        let mut entry = self
            .storage
            .get_bridge_journal(journal_key)?
            .ok_or_else(|| invalid("no recovery entry under this key"))?;
        if entry.complete || entry.transaction_hash.is_some() {
            return Err(invalid("recovery transaction already recorded"));
        }
        entry.transaction_hash = Some(hash.to_string());
        self.storage.put_bridge_journal(journal_key, entry)
    }

    /// Forget an unfinished recovery entry so the next run submits again. Only for
    /// a submission that never left this process, or whose transaction reverted:
    /// otherwise the next run can move the funds twice.
    pub async fn release_cctp_recovery(&self, journal_key: &str) -> Result<()> {
        let _guard = self.evm_execution.lock().await;
        let entry = self
            .storage
            .get_bridge_journal(journal_key)?
            .ok_or_else(|| invalid("no recovery entry under this key"))?;
        if entry.complete {
            return Err(invalid("a completed recovery cannot be released"));
        }
        self.storage.remove_bridge_journal(journal_key)
    }

    pub(super) fn cctp_recovery_recorded(&self, source_domain: u32, nonce: B256) -> Result<bool> {
        let key = recovery_key(source_domain, nonce);
        for key in [format!("{key}:mint"), format!("{key}:sweep"), key] {
            if self.storage.get_bridge_journal(&key)?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn select_recovery(
        &self,
        recovery: &CctpInboundRecovery<'_>,
        receipt: &EvmReceipt,
    ) -> Result<Selected> {
        if receipt.transaction_hash != recovery.source_txid {
            return Err(invalid("source receipt hash mismatch"));
        }
        let candidates: Vec<CctpBurn> = cctp::burns_from_receipt(receipt, &recovery.source)?
            .into_iter()
            .filter(|burn| {
                burn.destination_domain == recovery.destination.domain
                    && burn.destination_messenger == recovery.source.messenger.into_word()
            })
            .collect();
        if candidates.is_empty() {
            return Err(invalid(
                "the source transaction has no CCTP burn to this destination",
            ));
        }
        let mut selected = Vec::new();
        for burn in candidates {
            let account = Address::from_word(burn.mint_recipient);
            let controlled = match &recovery.account {
                CctpRecoveryAccount::External(wallet) => {
                    (wallet.address() == account).then_some(None)
                }
                CctpRecoveryAccount::Local { key_index, .. } => {
                    self.seed_key_for(account, *key_index)?.map(Some)
                }
            };
            if let Some(key) = controlled {
                selected.push(Selected { burn, account, key });
            }
        }
        if selected.len() > 1 {
            return Err(invalid(
                "several burns in the source transaction mint to this account",
            ));
        }
        selected.pop().ok_or_else(|| {
            Error::InvalidSigner(
                "the burn's mint recipient is not controlled by this seed or wallet".into(),
            )
        })
    }

    fn owned_swaps(&self, account: Address) -> Result<Vec<crate::StoredSwap>> {
        Ok(self
            .list_all_swaps()?
            .into_iter()
            .filter(|stored| evm_owner(&stored.response) == Some(account))
            .collect())
    }

    fn seed_key_for(&self, account: Address, hint: Option<u32>) -> Result<Option<EvmKey>> {
        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| Error::InvalidSigner("recovery needs the swap seed".into()))?;
        let address = account.to_string();
        let owned = self.owned_swaps(account)?;
        if !owned.is_empty() {
            // A stored owner with no seed key is an external signer's account.
            return Ok(owned
                .iter()
                .find_map(|stored| signer.evm_key_for_address(stored.key_index, &address).ok()));
        }
        let matches = |key: &EvmKey| key.address.eq_ignore_ascii_case(&address);
        let wallet = signer.derive_wallet_evm_key()?;
        if matches(&wallet) {
            return Ok(Some(wallet));
        }
        for index in hint.into_iter().chain(0..RECOVERY_SCAN) {
            for key in [
                signer.derive_evm_key(index)?,
                signer.derive_legacy_evm_key(index)?,
            ] {
                if matches(&key) {
                    return Ok(Some(key));
                }
            }
        }
        Ok(None)
    }

    /// Refuses while a stored swap on this account may be using this burn's USDC.
    /// Returns the owning swap when exactly one owns the account.
    fn recovery_guard(&self, account: Address, nonce: B256) -> Result<Option<String>> {
        let owned = self.owned_swaps(account)?;
        for stored in &owned {
            let id = stored.response.id();
            let metadata = &stored.metadata;
            let operation = |key: &str| {
                metadata
                    .bridge_operations
                    .as_ref()
                    .and_then(|ops| ops.get(key))
            };
            if ["cctp_fund", "cctp_mint"]
                .into_iter()
                .filter_map(operation)
                .any(|op| !op.complete)
            {
                return Err(invalid(format!(
                    "swap {id} has an unfinished CCTP operation on this account; resume it first"
                )));
            }
            let of_this_burn = |key: &str| {
                operation(key).is_some_and(|op| {
                    op.intent
                        .get("nonce")
                        .and_then(|value| value.as_str())
                        .and_then(|value| value.parse::<B256>().ok())
                        == Some(nonce)
                })
            };
            let funded = metadata.evm_fund_txid.is_some()
                || metadata.evm_fund_userop_hash.is_some()
                || metadata.evm_fund_started == Some(true);
            // On an account several swaps share (the wallet key), only a journaled
            // mint or funding of this burn ties a swap's funding to its USDC.
            if of_this_burn("cctp_fund")
                || (funded && (owned.len() == 1 || of_this_burn("cctp_mint")))
            {
                return Err(invalid(format!(
                    "swap {id} records funding that may have used this burn's USDC; refund the swap instead"
                )));
            }
        }
        Ok(match owned.as_slice() {
            [only] => Some(only.response.id().to_owned()),
            _ => None,
        })
    }

    pub(super) async fn execute_recovery(
        &self,
        api: &BridgeApi,
        recovery: &CctpInboundRecovery<'_>,
        selected: Selected,
        wallet: &dyn EvmWallet,
        options: &PollOptions,
    ) -> Result<CctpInboundRecovered> {
        let Selected { burn, account, key } = selected;
        let destination = &recovery.destination;
        if wallet.address() != account || wallet.chain_id() != destination.chain_id {
            return Err(invalid(
                "recovery wallet is not the mint recipient on the destination chain",
            ));
        }
        if recovery.recipient == account {
            return Err(invalid("recovery recipient is the minting account"));
        }
        let attestation = match &recovery.attestation {
            Some(attestation) => attestation.clone(),
            None => {
                api.wait_cctp_attestation(recovery.source_txid, &burn, options)
                    .await?
            }
        };
        let receive = attestation.receive_calldata(&burn)?;
        let message: Bytes = crate::evm::parse(&attestation.message)?;
        let decoded = burn.validate_message(&message)?;
        let minted = decoded.minted_amount();
        let journal_key = recovery_key(burn.source_domain, decoded.nonce);
        let mint_key = format!("{journal_key}:mint");
        let sweep_key = format!("{journal_key}:sweep");
        let intent = json!({"burn":burn,"destination":destination,"nonce":decoded.nonce,"account":account,"recipient":recovery.recipient,"amount":minted,"sweep":recovery.sweep});
        // A wallet mint moves nothing to the recipient, so its entry must not pin
        // the recipient or sweep of a transfer that has not been sent.
        let mint_intent =
            json!({"burn":burn,"destination":destination,"nonce":decoded.nonce,"account":account});
        let receive = Call {
            target: destination.transmitter,
            value: U256::ZERO,
            callData: receive,
        };
        let transfer = |amount: U256| Call {
            target: destination.usdc,
            value: U256::ZERO,
            callData: Token::transferCall {
                to: recovery.recipient,
                amount,
            }
            .abi_encode()
            .into(),
        };
        let minted_by = |receipt: &EvmReceipt| {
            received_message(
                receipt,
                destination.transmitter,
                &burn,
                decoded.nonce,
                &message,
            )
            .then_some(receipt.transaction_hash)
        };
        // A resumed full-balance sweep cannot know what the balance was.
        let expected = |amount: Option<U256>| match recovery.sweep {
            CctpSweep::Minted => Some(minted),
            CctpSweep::FullBalance => amount,
        };
        let recorded_mint = || -> Result<Option<B256>> {
            self.storage
                .get_bridge_journal(&mint_key)?
                .and_then(|entry| entry.transaction_hash)
                .map(|hash| crate::evm::parse(&hash))
                .transpose()
        };
        let mode = &recovery.submission;

        let _guard = self.evm_execution.lock().await;
        let swap_id = self.recovery_guard(account, decoded.nonce)?;
        // Each mode journals its transfer under its own key, and neither resumes the
        // other's. A completed wallet mint with no sweep entry sent nothing onward.
        let journaled = |key: &str| self.storage.get_bridge_journal(key);
        let other_mode = match mode {
            BridgeSubmission::UserOperation(_) => {
                if journaled(&sweep_key)?.is_some() {
                    Some(&sweep_key)
                } else if journaled(&mint_key)?.is_some_and(|entry| !entry.complete) {
                    Some(&mint_key)
                } else {
                    None
                }
            }
            BridgeSubmission::Wallet => journaled(&journal_key)?.map(|_| &journal_key),
        };
        if let Some(key) = other_mode {
            return Err(invalid(format!(
                "this burn's recovery is journaled under {key} by the other submission mode; resume it in that mode"
            )));
        }
        let (sweep, mint_transaction, amount) = match mode {
            BridgeSubmission::UserOperation(_) => {
                let journal = Journal::Burn {
                    key: &journal_key,
                    signer: key.as_ref(),
                };
                let (receipt, amount) = match self
                    .bridge_receipt(journal, &intent, wallet, mode)
                    .await?
                {
                    Some(receipt) => (receipt, None),
                    None => {
                        let mint = self
                            .mint_needed(wallet, destination, &burn, &decoded)
                            .await?;
                        let balance = token_balance(wallet, destination.usdc).await?;
                        let amount = sweep_amount(recovery.sweep, balance, minted, mint)?;
                        let mut calls = Vec::new();
                        if mint {
                            calls.push(receive);
                        }
                        calls.push(transfer(amount));
                        let receipt = self
                            .submit_bridge(journal, intent, wallet, mode, calls)
                            .await?;
                        if mint && minted_by(&receipt).is_none() {
                            return Err(invalid("receipt does not contain the approved CCTP mint"));
                        }
                        (receipt, Some(amount))
                    }
                };
                let amount = transferred(
                    &receipt,
                    destination.usdc,
                    account,
                    recovery.recipient,
                    expected(amount),
                )?;
                self.complete_bridge(journal)?;
                let mint_transaction = match minted_by(&receipt) {
                    Some(hash) => Some(hash),
                    None => recorded_mint()?,
                };
                (receipt, mint_transaction, amount)
            }
            BridgeSubmission::Wallet => {
                let mint_journal = Journal::Burn {
                    key: &mint_key,
                    signer: None,
                };
                let sweep_journal = Journal::Burn {
                    key: &sweep_key,
                    signer: None,
                };
                let (receipt, amount) = match self
                    .bridge_receipt(sweep_journal, &intent, wallet, mode)
                    .await?
                {
                    Some(receipt) => (receipt, None),
                    None => {
                        let minting = match self
                            .bridge_receipt(mint_journal, &mint_intent, wallet, mode)
                            .await?
                        {
                            Some(receipt) => Some(receipt),
                            None => {
                                if self
                                    .mint_needed(wallet, destination, &burn, &decoded)
                                    .await?
                                {
                                    let calls = vec![receive];
                                    Some(
                                        self.submit_bridge(
                                            mint_journal,
                                            mint_intent,
                                            wallet,
                                            mode,
                                            calls,
                                        )
                                        .await?,
                                    )
                                } else {
                                    None
                                }
                            }
                        };
                        if let Some(receipt) = minting {
                            if minted_by(&receipt).is_none() {
                                return Err(invalid(
                                    "receipt does not contain the approved CCTP mint",
                                ));
                            }
                            self.complete_bridge(mint_journal)?;
                        }
                        let balance = token_balance(wallet, destination.usdc).await?;
                        let amount = sweep_amount(recovery.sweep, balance, minted, false)?;
                        let receipt = self
                            .submit_bridge(
                                sweep_journal,
                                intent,
                                wallet,
                                mode,
                                vec![transfer(amount)],
                            )
                            .await?;
                        (receipt, Some(amount))
                    }
                };
                let amount = transferred(
                    &receipt,
                    destination.usdc,
                    account,
                    recovery.recipient,
                    expected(amount),
                )?;
                self.complete_bridge(sweep_journal)?;
                (receipt, recorded_mint()?, amount)
            }
        };
        Ok(CctpInboundRecovered {
            burn,
            nonce: decoded.nonce,
            account,
            swap_id,
            journal_key,
            mint_transaction,
            sweep,
            recovered_amount: amount,
        })
    }

    /// Whether the nonce is still unused, in which case only the destination
    /// caller can mint it.
    async fn mint_needed(
        &self,
        wallet: &dyn EvmWallet,
        destination: &CctpDestination,
        burn: &CctpBurn,
        decoded: &cctp::CctpMessage,
    ) -> Result<bool> {
        let used = word(
            &wallet
                .call(&transaction(
                    wallet,
                    destination.transmitter,
                    cctp::usedNoncesCall {
                        nonce: decoded.nonce,
                    }
                    .abi_encode(),
                ))
                .await?,
        )?;
        if used > U256::ONE {
            return Err(invalid("invalid CCTP nonce state"));
        }
        if used.is_zero()
            && !burn.destination_caller.is_zero()
            && burn.destination_caller != wallet.address().into_word()
        {
            return Err(invalid(
                "the burn names another destination caller; only it can mint",
            ));
        }
        Ok(used.is_zero())
    }
}

fn evm_owner(response: &crate::types::GetSwapResponse) -> Option<Address> {
    use crate::types::GetSwapResponse::*;
    match response {
        EvmToBitcoin(r) => r.client_evm_address.parse().ok(),
        EvmToArkade(r) => r.client_evm_address.parse().ok(),
        EvmToLightning(r) => r.client_evm_address.parse().ok(),
        BitcoinToEvm(r) => r.client_evm_address.parse().ok(),
        ArkadeToEvm(r) => r.client_evm_address.parse().ok(),
        LightningToEvm(r) => r.client_evm_address.parse().ok(),
        _ => None,
    }
}

/// `balance` is read before a mint that the same batch performs, if `minting`.
pub(super) fn sweep_amount(
    sweep: CctpSweep,
    balance: U256,
    minted: U256,
    minting: bool,
) -> Result<U256> {
    match (sweep, minting) {
        (CctpSweep::Minted, true) => Ok(minted),
        (CctpSweep::Minted, false) if balance >= minted => Ok(minted),
        (CctpSweep::Minted, false) => Err(invalid("the minted USDC is no longer on the account")),
        (CctpSweep::FullBalance, true) => balance
            .checked_add(minted)
            .ok_or_else(|| invalid("balance overflow")),
        (CctpSweep::FullBalance, false) if !balance.is_zero() => Ok(balance),
        (CctpSweep::FullBalance, false) => Err(invalid("no USDC on the account to recover")),
    }
}
