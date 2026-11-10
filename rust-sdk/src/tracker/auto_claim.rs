//! Stock claim executor, the counterpart of the TypeScript client's `withAutoClaim`.
use super::ActionExecutor;
use super::ExecutionFuture;
use super::ExecutionResult;
use crate::Client;
use crate::Error;
use crate::Result;
use crate::StoredSwap;
use crate::SwapMetadata;
use crate::actions::ActionKind;
use crate::arkade::ArkadeBackend;
use crate::arkade::ArkadeSettlement;
use crate::arkade::ArkadeSpend;
use crate::bitcoin::BitcoinBackend;
use crate::bitcoin::BitcoinSpendOptions;
#[cfg(feature = "gasless")]
use crate::logging::sdk_log;
use crate::types::GetSwapResponse;
use bitcoin::Amount;
use std::sync::Arc;
use std::time::Duration;

type DestinationFn = dyn Fn(&StoredSwap) -> Option<String> + Send + Sync;

/// A rail left out is refused. The tracker retries a refusal like any execution
/// that sent nothing, then hands the claim to the user.
#[derive(Clone)]
pub struct AutoClaimConfig {
    /// EVM to Bitcoin claims.
    pub bitcoin: Option<BitcoinAutoClaim>,
    /// Claims of EVM-target swaps. Arkade claims use the client's Arkade config.
    #[cfg(feature = "gasless")]
    pub evm: Option<EvmAutoClaim>,
    /// Destination of a swap whose creation request was not recorded locally, such
    /// as a recovered one. Its answer is used as given.
    pub destination: Option<Arc<DestinationFn>>,
    /// Deadline of one claim. It must be shorter than
    /// `TrackerOptions::execution_timeout`, so that the claimer, not the tracker,
    /// decides whether a slow claim sent anything.
    pub timeout: Duration,
}
impl Default for AutoClaimConfig {
    fn default() -> Self {
        Self {
            bitcoin: None,
            #[cfg(feature = "gasless")]
            evm: None,
            destination: None,
            timeout: Duration::from_secs(90),
        }
    }
}

#[derive(Clone)]
pub struct BitcoinAutoClaim {
    pub backend: Arc<dyn BitcoinBackend>,
    /// The fee rate is the backend's estimate for `confirmation_target`.
    pub maximum_fee: Amount,
    pub minimum_remaining_blocks: u32,
    pub confirmation_target: u16,
}
impl BitcoinAutoClaim {
    pub fn new(backend: Arc<dyn BitcoinBackend>, maximum_fee: Amount) -> Self {
        Self {
            backend,
            maximum_fee,
            minimum_remaining_blocks: 1,
            confirmation_target: 6,
        }
    }
}

#[cfg(feature = "gasless")]
#[derive(Clone, Debug)]
pub struct EvmAutoClaim {
    pub chains: std::collections::HashMap<u64, EvmAutoClaimChain>,
    /// How far a relayed payout, or a sponsored DEX payout, may fall below
    /// `target_amount`, in basis points. None uses the daemon's own floor: 200 for
    /// Bitcoin to EVM, 100 otherwise.
    pub slippage_bps: Option<u16>,
    pub minimum_remaining_seconds: u64,
    /// Circle's IRIS API, for the CCTP fee of a payout bridged from Arbitrum to
    /// another EVM chain (USDC on Polygon, Ethereum, Base, ...). Defaults to
    /// `BridgeApi::mainnet()`; test networks pass their own. `None` refuses such
    /// payouts, which then need a manual, reviewed bridge claim.
    pub bridge_api: Option<crate::bridge::BridgeApi>,
}
#[cfg(feature = "gasless")]
impl Default for EvmAutoClaim {
    fn default() -> Self {
        Self {
            chains: Default::default(),
            slippage_bps: None,
            minimum_remaining_seconds: 300,
            // Payouts in USDC on Polygon and Ethereum bridge over CCTP; without
            // the fee API they would not be claimed automatically.
            bridge_api: crate::bridge::BridgeApi::mainnet().ok(),
        }
    }
}

#[cfg(feature = "gasless")]
#[derive(Clone, Debug)]
pub struct EvmAutoClaimChain {
    pub rpc_url: url::Url,
    /// The contracts a claim may sign for: the daemon's current and legacy
    /// deployments on this chain. The server does not publish them and the SDK
    /// ships none, so they come from the application.
    pub deployments: Vec<EvmDeployment>,
    /// Claims direct payouts, and DEX payouts on Arbitrum, with a sponsored
    /// UserOperation. A direct payout whose operation sent nothing falls back to
    /// the server relay.
    pub user_op: Option<crate::aa::UserOpConfig>,
}

#[cfg(feature = "gasless")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvmDeployment {
    pub htlc: crate::evm::Address,
    pub coordinator: crate::evm::Address,
}

#[cfg(feature = "gasless")]
const ARBITRUM: u64 = 42161;

/// Claims the swaps the tracker hands it, choosing the rail by direction. The
/// destination is the one recorded when the swap was created, and EVM contracts
/// must be in the configured allowlist, so neither comes from the server alone.
/// A failed claim is `NotSent`, and so retried, only while its rail's submission
/// journal is still clear; everything else is left for reconciliation.
pub struct AutoClaimer {
    client: Client,
    config: AutoClaimConfig,
    /// Test seams for the RPC wallet and the arkd connection.
    #[cfg(feature = "gasless")]
    pub(crate) evm_wallet: Option<Arc<dyn crate::evm::EvmWallet>>,
    pub(crate) arkade: Option<Arc<dyn ArkadeBackend>>,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidSwap(message.into())
}

/// Set just before a claim can leave the process, by every rail.
fn reserved(metadata: &SwapMetadata) -> bool {
    metadata.evm_claim_started == Some(true)
        || metadata.evm_claim_txid.is_some()
        || metadata.evm_claim_userop_hash.is_some()
        || metadata.btc_spend_txid.is_some()
        || metadata.arkade_pending.is_some()
        || metadata.arkade_delegate.is_some()
}

impl AutoClaimer {
    pub fn new(client: Client, config: AutoClaimConfig) -> Result<Self> {
        if client.signer.is_none() {
            return Err(Error::InvalidSigner(
                "automatic claims require the swap seed".into(),
            ));
        }
        if config.timeout.is_zero() {
            return Err(invalid("invalid automatic claim bounds"));
        }
        #[cfg(feature = "gasless")]
        if config
            .evm
            .as_ref()
            .and_then(|e| e.slippage_bps)
            .is_some_and(|bps| bps >= 10_000)
        {
            return Err(invalid("slippage must be below 10000 bps"));
        }
        Ok(Self {
            client,
            config,
            #[cfg(feature = "gasless")]
            evm_wallet: None,
            arkade: None,
        })
    }

    fn may_have_sent(&self, id: &str) -> bool {
        !matches!(self.client.get_stored_swap(id), Ok(Some(stored)) if !reserved(&stored.metadata))
    }

    /// The locally recorded destination, which must also be the server's.
    fn destination(&self, stored: &StoredSwap) -> Result<String> {
        use GetSwapResponse::*;
        let (server, evm) = match &stored.response {
            BtcToArkade(r) => (Some(&r.target_arkade_address), false),
            EvmToArkade(r) => (Some(&r.target_arkade_address), false),
            LightningToArkade(r) => (Some(&r.target_arkade_address), false),
            EvmToBitcoin(r) => (r.target_btc_address.as_ref(), false),
            BitcoinToEvm(r) => (r.target_evm_address.as_ref(), true),
            ArkadeToEvm(r) => (r.target_evm_address.as_ref(), true),
            LightningToEvm(r) => (r.target_evm_address.as_ref(), true),
            EvmToLightning(_) | ArkadeToLightning(_) | BitcoinToLightning(_) => {
                return Err(invalid("the server claims this direction"));
            }
        };
        let Some(local) = &stored.metadata.target_address else {
            return self
                .config
                .destination
                .as_ref()
                .and_then(|destination| destination(stored))
                .ok_or_else(|| {
                    invalid(
                        "no destination was recorded at creation; set AutoClaimConfig::destination",
                    )
                });
        };
        if !server.is_some_and(|server| {
            if evm {
                server.eq_ignore_ascii_case(local)
            } else {
                server == local
            }
        }) {
            return Err(invalid(
                "the recorded destination differs from the server's; claim manually",
            ));
        }
        Ok(local.clone())
    }

    /// `None` when the claim was accepted without a reference to follow.
    async fn claim(&self, id: &str, stored: &StoredSwap) -> Result<Option<String>> {
        use GetSwapResponse::*;
        let destination = self.destination(stored)?;
        match &stored.response {
            BtcToArkade(_) | EvmToArkade(_) | LightningToArkade(_) => {
                self.claim_arkade(id, destination).await.map(Some)
            }
            EvmToBitcoin(_) => self.claim_bitcoin(id, stored, destination).await.map(Some),
            BitcoinToEvm(_) | ArkadeToEvm(_) | LightningToEvm(_) => {
                self.claim_evm(id, stored, &destination).await
            }
            EvmToLightning(_) | ArkadeToLightning(_) | BitcoinToLightning(_) => {
                Err(invalid("the server claims this direction"))
            }
        }
    }

    async fn claim_arkade(&self, id: &str, destination: String) -> Result<String> {
        let backend: &dyn ArkadeBackend = match &self.arkade {
            Some(backend) => backend.as_ref(),
            None => self.client.arkade_wallet().await?,
        };
        let settlement = self
            .client
            .settle_with(
                id,
                backend,
                ArkadeSpend::Claim,
                destination,
                self.config.timeout,
            )
            .await?;
        Ok(match settlement {
            ArkadeSettlement::Offchain(receipt) => receipt.ark_txid.to_string(),
            ArkadeSettlement::Delegated { commitment_txid } => commitment_txid.to_string(),
        })
    }

    async fn claim_bitcoin(
        &self,
        id: &str,
        stored: &StoredSwap,
        destination: String,
    ) -> Result<String> {
        let config = self
            .config
            .bitcoin
            .as_ref()
            .ok_or_else(|| invalid("automatic Bitcoin claims are not configured"))?;
        // The depth the tracker read the funding at, so the claim it recommended
        // spends the outputs it saw.
        let minimum_confirmations = stored
            .metadata
            .tracking
            .clone()
            .and_then(|value| serde_json::from_value::<super::TrackingRecord>(value).ok())
            .and_then(|mut record| {
                record.upgrade();
                match record.contracts.server {
                    Some(crate::observation::ContractRef::Bitcoin { confirmations, .. }) => {
                        confirmations
                    }
                    _ => None,
                }
            })
            .or_else(|| self.client.bitcoin_min_confirmations())
            .unwrap_or(0);
        let receipt = self
            .client
            .claim_bitcoin(
                id,
                config.backend.as_ref(),
                BitcoinSpendOptions {
                    destination,
                    // A set rate lets a replacement broadcast before it is journaled.
                    fee_rate: None,
                    maximum_fee: config.maximum_fee,
                    minimum_confirmations,
                    minimum_remaining_blocks: config.minimum_remaining_blocks,
                    confirmation_target: config.confirmation_target,
                    timeout: self.config.timeout,
                },
            )
            .await?;
        Ok(receipt.transaction.compute_txid().to_string())
    }

    #[cfg(not(feature = "gasless"))]
    async fn claim_evm(&self, _: &str, _: &StoredSwap, _: &str) -> Result<Option<String>> {
        Err(invalid("EVM claims require the gasless feature"))
    }

    #[cfg(feature = "gasless")]
    async fn claim_evm(
        &self,
        id: &str,
        stored: &StoredSwap,
        destination: &str,
    ) -> Result<Option<String>> {
        use crate::evm::Address;
        use crate::evm::U256;
        let config = self
            .config
            .evm
            .as_ref()
            .ok_or_else(|| invalid("automatic EVM claims are not configured"))?;
        let value =
            serde_json::to_value(&stored.response).map_err(|e| Error::Decode(e.to_string()))?;
        let text = |name: &str| {
            value[name]
                .as_str()
                .ok_or_else(|| invalid(format!("missing {name}")))
        };
        let address = |text: &str| {
            text.parse::<Address>()
                .map_err(|e| Error::Decode(e.to_string()))
        };
        let amount =
            |text: &str| U256::from_str_radix(text, 10).map_err(|e| Error::Decode(e.to_string()));
        // The daemon builds bridge calls with no minimum output, so a bridged payout
        // is claimed only when the SDK builds the bridge leg itself: CCTP from
        // Arbitrum to an EVM chain it knows.
        let cctp_destination = match value["bridge_target_chain"].as_str() {
            None if value["bridge_target_chain"].is_null() => None,
            name => Some(
                name.and_then(crate::bridge::cctp::CctpChain::for_bridge_name)
                    .ok_or_else(|| {
                        invalid(
                            "bridged payouts are not claimed automatically; claim with a reviewed bridge plan",
                        )
                    })?,
            ),
        };
        let context = crate::evm::EvmContext::from_response(&stored.response)?;
        let chain = config
            .chains
            .get(&context.chain_id)
            .ok_or_else(|| invalid("automatic claims are not configured for this EVM chain"))?;
        let coordinator = address(text("evm_coordinator_address")?)?;
        if !chain.deployments.contains(&EvmDeployment {
            htlc: context.htlc,
            coordinator,
        }) {
            return Err(invalid(
                "the swap's HTLC and coordinator are not an allowed deployment",
            ));
        }
        let target_token = address(
            value["target_token"]["token_id"]
                .as_str()
                .ok_or_else(|| invalid("missing target token"))?,
        )?;
        if let Some(destination_chain) = cctp_destination {
            return self
                .claim_evm_cctp(
                    id,
                    stored,
                    &value,
                    destination,
                    destination_chain,
                    context,
                    coordinator,
                    target_token,
                    chain,
                    config,
                )
                .await
                .map(Some);
        }
        let direct = target_token == context.source_token;
        let slippage_bps = config.slippage_bps.unwrap_or(
            if matches!(stored.response, GetSwapResponse::BitcoinToEvm(_)) {
                200
            } else {
                100
            },
        );
        // The daemon's relay minimum for every payout without a bridge, direct ones
        // included. claim_evm_gasless refuses server calls on a direct payout, so the
        // gap below the locked amount cannot be diverted.
        let floor = amount(text("target_amount")?)?
            .checked_mul(U256::from(10_000 - slippage_bps))
            .ok_or_else(|| invalid("target amount overflow"))?
            / U256::from(10_000);
        let wallet: Arc<dyn crate::evm::EvmWallet> = match &self.evm_wallet {
            Some(wallet) => wallet.clone(),
            None => {
                let key = self
                    .client
                    .signer
                    .as_ref()
                    .ok_or_else(|| Error::InvalidSigner("claims require the swap seed".into()))?
                    .evm_key_for_address(stored.key_index, &context.owner.to_string())?;
                Arc::new(
                    crate::evm::RpcWallet::connect(&key, context.chain_id, chain.rpc_url.clone())
                        .await?,
                )
            }
        };
        let options = crate::evm::EvmClaimOptions {
            htlc: context.htlc,
            coordinator,
            destination: address(destination)?,
            sweep_token: if direct {
                context.source_token
            } else {
                target_token
            },
            min_amount_out: floor,
            minimum_remaining_seconds: config.minimum_remaining_seconds,
            timeout: self.config.timeout,
        };
        let arbitrum = context.chain_id == ARBITRUM;
        match &chain.user_op {
            Some(user_op) if direct || arbitrum => {
                let mut sponsored = options.clone();
                let calls = if direct {
                    // With no calls the whole locked amount reaches the destination.
                    sponsored.min_amount_out = amount(text("evm_expected_sats")?)?;
                    vec![]
                } else {
                    use crate::types::DexAmount;
                    use crate::types::DexCallsPayload;
                    use crate::types::DexToken;
                    let response = self
                        .client
                        .get_dex_calldata_fund(crate::types::DexCalldataRequest {
                            from: DexToken::Evm {
                                chain_id: context.chain_id,
                                address: context.source_token.to_string(),
                            },
                            to: DexToken::Evm {
                                chain_id: context.chain_id,
                                address: target_token.to_string(),
                            },
                            amount: DexAmount::ExactIn(text("evm_expected_sats")?.to_owned()),
                            sender: coordinator.to_string(),
                            slippage_bps,
                            bridge_recipient: None,
                        })
                        .await?;
                    let DexCallsPayload::Evm { calls } = response.payload;
                    calls
                };
                match self
                    .client
                    .claim_evm_userop(id, wallet.as_ref(), sponsored, calls, user_op)
                    .await
                {
                    Ok(receipt) => {
                        return Ok(Some(
                            receipt
                                .transaction_hash
                                .unwrap_or(receipt.user_op_hash)
                                .to_string(),
                        ));
                    }
                    // A DEX payout on Arbitrum has no working server path.
                    Err(error) if !direct || self.may_have_sent(id) => return Err(error),
                    Err(error) => {
                        sdk_log!(
                            self.client.log.clone().swap_id(id),
                            Warn,
                            "client.claim.sponsoredFallback",
                            "sponsored claim sent nothing; claiming through the server relay",
                            "error" => error.to_string()
                        );
                    }
                }
            }
            None if arbitrum && !direct => {
                return Err(invalid(
                    "an Arbitrum DEX payout requires a UserOperation configuration",
                ));
            }
            _ => {}
        }
        let submitted = self
            .client
            .claim_evm_gasless(id, wallet.as_ref(), options)
            .await?;
        Ok((!submitted.tx_hash.is_empty()).then_some(submitted.tx_hash))
    }

    /// Claims a payout bridged over CCTP from Arbitrum to another EVM chain. The
    /// server supplies only the Arbitrum DEX calls; the SDK builds and pins the
    /// bridge leg itself (adapter, token, domain, recipient, fee cap) and asserts
    /// on chain, before the burn, that the DEX left at least the payout floor.
    #[cfg(feature = "gasless")]
    #[allow(clippy::too_many_arguments)]
    async fn claim_evm_cctp(
        &self,
        id: &str,
        stored: &StoredSwap,
        value: &serde_json::Value,
        destination: &str,
        destination_chain: crate::bridge::cctp::CctpChain,
        context: crate::evm::EvmContext,
        coordinator: crate::evm::Address,
        target_token: crate::evm::Address,
        chain: &EvmAutoClaimChain,
        config: &EvmAutoClaim,
    ) -> Result<String> {
        use crate::bridge::BridgeSubmission;
        use crate::bridge::OutboundReceipt;
        use crate::bridge::cctp::CctpChain;
        use crate::evm::Address;
        use crate::evm::U256;
        use crate::types::DexAmount;
        use crate::types::DexCallsPayload;
        use crate::types::DexToken;
        let refuse = |reason: &str| {
            invalid(format!(
                "bridged payout is not claimed automatically: {reason}; claim with a reviewed bridge plan"
            ))
        };
        let arbitrum = CctpChain::for_chain(ARBITRUM)?;
        if context.chain_id != ARBITRUM || target_token != arbitrum.usdc {
            return Err(refuse("only Arbitrum USDC payouts are bridged by the SDK"));
        }
        let expected_token = value["bridge_target_token_address"]
            .as_str()
            .and_then(|text| text.parse::<Address>().ok());
        if expected_token != Some(destination_chain.usdc) {
            return Err(refuse(
                "the bridged token is not native USDC on the destination",
            ));
        }
        let api = config
            .bridge_api
            .as_ref()
            .ok_or_else(|| refuse("no CCTP fee API is configured"))?;
        let user_op = chain.user_op.as_ref().ok_or_else(|| {
            refuse("an Arbitrum DEX payout requires a UserOperation configuration")
        })?;
        let recipient = destination
            .parse::<Address>()
            .map_err(|e| Error::Decode(e.to_string()))?;
        let amount =
            |text: &str| U256::from_str_radix(text, 10).map_err(|e| Error::Decode(e.to_string()));
        let text = |name: &str| {
            value[name]
                .as_str()
                .ok_or_else(|| invalid(format!("missing {name}")))
        };
        let slippage_bps = config.slippage_bps.unwrap_or(
            if matches!(stored.response, GetSwapResponse::BitcoinToEvm(_)) {
                200
            } else {
                100
            },
        );
        // The same floor the daemon relays any other payout against.
        let minimum = amount(text("target_amount")?)?
            .checked_mul(U256::from(10_000 - slippage_bps))
            .ok_or_else(|| invalid("target amount overflow"))?
            / U256::from(10_000);
        let response = self
            .client
            .get_dex_calldata_fund(crate::types::DexCalldataRequest {
                from: DexToken::Evm {
                    chain_id: ARBITRUM,
                    address: context.source_token.to_string(),
                },
                to: DexToken::Evm {
                    chain_id: ARBITRUM,
                    address: arbitrum.usdc.to_string(),
                },
                amount: DexAmount::ExactIn(text("evm_expected_sats")?.to_owned()),
                sender: coordinator.to_string(),
                slippage_bps,
                bridge_recipient: None,
            })
            .await?;
        let estimated = amount(&response.estimated_amount_out)?;
        let DexCallsPayload::Evm { calls } = response.payload;
        let fee = api
            .cctp_fee(arbitrum.domain, destination_chain.domain, 1000)
            .await?;
        let claim = cctp_payout_claim(
            calls,
            estimated,
            minimum,
            &fee,
            destination_chain,
            recipient,
            coordinator,
        )?;
        let wallet: Arc<dyn crate::evm::EvmWallet> = match &self.evm_wallet {
            Some(wallet) => wallet.clone(),
            None => {
                let key = self
                    .client
                    .signer
                    .as_ref()
                    .ok_or_else(|| Error::InvalidSigner("claims require the swap seed".into()))?
                    .evm_key_for_address(stored.key_index, &context.owner.to_string())?;
                Arc::new(
                    crate::evm::RpcWallet::connect(&key, context.chain_id, chain.rpc_url.clone())
                        .await?,
                )
            }
        };
        let options = crate::evm::EvmClaimOptions {
            htlc: context.htlc,
            coordinator,
            destination: recipient,
            sweep_token: arbitrum.usdc,
            // The bridge consumes the whole balance; the floor is asserted in the calls.
            min_amount_out: U256::ZERO,
            minimum_remaining_seconds: config.minimum_remaining_seconds,
            timeout: self.config.timeout,
        };
        let receipt = self
            .client
            .claim_evm_to_bridge(
                id,
                wallet.as_ref(),
                options,
                claim,
                BridgeSubmission::UserOperation(user_op),
            )
            .await?;
        match receipt {
            OutboundReceipt::Cctp { source_txid, .. } => Ok(source_txid.to_string()),
            OutboundReceipt::Usdt0(_) => Err(invalid("expected a CCTP receipt")),
        }
    }
}

#[cfg(feature = "gasless")]
alloy::sol! {
    /// An ERC-20 self-transfer by the coordinator asserts its balance.
    interface FloorCheck {
        function transfer(address to, uint256 amount) external returns (bool);
    }
}

/// The SDK's own plan for a CCTP payout from Arbitrum: the server's DEX calls,
/// then a USDC self-transfer of the floor by the coordinator, which reverts
/// unless the DEX left at least that much, then the pinned adapter burn.
///
/// `estimated` is the DEX's expected output (the burn the plan expects); the
/// burn is only bounded from below, by `minimum`, which must exceed the fee cap
/// so the recipient always receives something.
#[cfg(feature = "gasless")]
pub(crate) fn cctp_payout_claim(
    mut dex_calls: Vec<crate::types::DexCall>,
    estimated: crate::evm::U256,
    minimum: crate::evm::U256,
    fee: &crate::bridge::cctp::CctpFee,
    destination: crate::bridge::cctp::CctpChain,
    recipient: crate::evm::Address,
    coordinator: crate::evm::Address,
) -> Result<crate::bridge::OutboundClaim> {
    use crate::bridge::OutboundBridge;
    use crate::bridge::OutboundClaim;
    use crate::bridge::cctp;
    use alloy::sol_types::SolCall;
    let arbitrum = cctp::CctpChain::for_chain(ARBITRUM)?;
    if destination.chain_id == ARBITRUM || recipient.is_zero() || coordinator.is_zero() {
        return Err(invalid("invalid CCTP payout destination"));
    }
    let amount = estimated.max(minimum);
    let max_fee = fee.max_fee(amount, true)?;
    if minimum <= max_fee {
        return Err(invalid(format!(
            "payout floor ({minimum}) does not exceed the CCTP fee cap ({max_fee}); amount too small to bridge"
        )));
    }
    // Reverts on a short balance, so a DEX leg that under-delivers can't be bridged.
    dex_calls.push(crate::types::DexCall {
        target: arbitrum.usdc.to_string(),
        data: format!(
            "0x{}",
            hex::encode(
                FloorCheck::transferCall {
                    to: coordinator,
                    amount: minimum,
                }
                .abi_encode()
            )
        ),
        value: "0".into(),
    });
    Ok(OutboundClaim {
        dex_calls,
        bridge: OutboundBridge::Cctp {
            burn: cctp::CctpBurn {
                source_chain_id: ARBITRUM,
                source_domain: arbitrum.domain,
                destination_domain: destination.domain,
                source_messenger: cctp::TOKEN_MESSENGER,
                destination_messenger: cctp::TOKEN_MESSENGER.into_word(),
                source_transmitter: cctp::MESSAGE_TRANSMITTER,
                burn_token: arbitrum.usdc,
                sender: cctp::ARBITRUM_ADAPTER,
                mint_recipient: recipient.into_word(),
                destination_caller: crate::evm::B256::ZERO,
                amount,
                max_fee,
                min_finality: 1000,
                hook: cctp::forwarding_hook(None),
            },
            minimum_amount: minimum,
        },
    })
}

impl ActionExecutor for AutoClaimer {
    fn execute<'a>(&'a self, id: &'a str, action: &'a ActionKind) -> ExecutionFuture<'a> {
        Box::pin(async move {
            if *action != ActionKind::Claim {
                return Ok(ExecutionResult::NotSent(invalid(
                    "the automatic claimer only claims",
                )));
            }
            let stored = match self.client.get_stored_swap(id) {
                Ok(Some(stored)) => stored,
                Ok(None) => return Ok(ExecutionResult::NotSent(invalid("swap record missing"))),
                Err(error) => return Ok(ExecutionResult::NotSent(error)),
            };
            if reserved(&stored.metadata) {
                return Err(invalid(
                    "a claim submission is already recorded; reconcile it first",
                ));
            }
            let result = tokio::time::timeout(self.config.timeout, self.claim(id, &stored))
                .await
                .unwrap_or_else(|_| Err(Error::Timeout("automatic claim timed out".into())));
            match result {
                Ok(Some(reference)) => Ok(ExecutionResult::Submitted { reference }),
                Ok(None) => Ok(ExecutionResult::Uncertain),
                // Error kinds cannot tell: a relay can answer 4xx after sending the claim.
                Err(error) if self.may_have_sent(id) => Err(error),
                Err(error) => Ok(ExecutionResult::NotSent(error)),
            }
        })
    }
    fn timeout(&self) -> Option<Duration> {
        Some(self.config.timeout)
    }
}

#[cfg(all(test, feature = "gasless"))]
mod cctp_payout_tests {
    use super::FloorCheck;
    use super::cctp_payout_claim;
    use crate::bridge::OutboundBridge;
    use crate::bridge::cctp;
    use crate::evm::Address;
    use crate::evm::U256;
    use alloy::sol_types::SolCall;

    fn fee() -> cctp::CctpFee {
        serde_json::from_value(serde_json::json!({
            "finalityThreshold": 1000,
            "minimumFee": 1.4,
            "forwardFee": {"high": 1_162_129},
        }))
        .unwrap()
    }
    fn dex_call() -> crate::types::DexCall {
        crate::types::DexCall {
            target: Address::repeat_byte(7).to_string(),
            data: "0x01".into(),
            value: "0".into(),
        }
    }

    #[test]
    fn bridge_names_resolve_only_to_evm_cctp_chains() {
        assert_eq!(
            cctp::CctpChain::for_bridge_name("Polygon").unwrap().domain,
            7
        );
        assert_eq!(
            cctp::CctpChain::for_bridge_name("ethereum").unwrap().domain,
            0
        );
        assert_eq!(
            cctp::CctpChain::for_bridge_name("World Chain")
                .unwrap()
                .chain_id,
            480
        );
        for refused in ["Solana", "Tron", "Plasma", "XDC", "Arbitrum ", ""] {
            assert!(
                cctp::CctpChain::for_bridge_name(refused).is_none(),
                "{refused}"
            );
        }
    }

    #[test]
    fn plan_pins_the_route_and_asserts_the_floor_before_the_burn() {
        let destination = cctp::CctpChain::for_bridge_name("Ethereum").unwrap();
        let recipient = Address::repeat_byte(0x11);
        let coordinator = Address::repeat_byte(0x22);
        let minimum = U256::from(9_000_000);
        let estimated = U256::from(10_000_000);
        let claim = cctp_payout_claim(
            vec![dex_call()],
            estimated,
            minimum,
            &fee(),
            destination,
            recipient,
            coordinator,
        )
        .unwrap();
        let arbitrum = cctp::CctpChain::for_chain(42161).unwrap();
        let OutboundBridge::Cctp {
            burn,
            minimum_amount,
        } = &claim.bridge
        else {
            panic!("expected a CCTP bridge")
        };
        burn.validate().unwrap();
        assert_eq!(*minimum_amount, minimum);
        assert_eq!(burn.source_chain_id, 42161);
        assert_eq!(burn.source_domain, arbitrum.domain);
        assert_eq!(burn.destination_domain, 0);
        assert_eq!(burn.burn_token, arbitrum.usdc);
        assert_eq!(burn.sender, cctp::ARBITRUM_ADAPTER);
        assert_eq!(burn.mint_recipient, recipient.into_word());
        assert!(burn.destination_caller.is_zero());
        assert_eq!(burn.min_finality, 1000);
        assert_eq!(burn.hook, cctp::forwarding_hook(None));
        assert_eq!(burn.amount, estimated);
        // 1.4 bps of 10 USDC with the 20% buffer, plus the forwarding fee.
        assert_eq!(burn.max_fee, fee().max_fee(estimated, true).unwrap());
        assert!(burn.max_fee < minimum);
        assert_eq!(claim.dex_calls.len(), 2);
        assert_eq!(claim.dex_calls[0], dex_call());
        let check = &claim.dex_calls[1];
        assert_eq!(check.target, arbitrum.usdc.to_string());
        assert_eq!(check.value, "0");
        let data = hex::decode(check.data.trim_start_matches("0x")).unwrap();
        let decoded = FloorCheck::transferCall::abi_decode(&data).unwrap();
        assert_eq!(decoded.to, coordinator);
        assert_eq!(decoded.amount, minimum);
    }

    #[test]
    fn plan_bounds_the_burn_by_the_floor_when_the_estimate_is_lower() {
        let destination = cctp::CctpChain::for_bridge_name("Polygon").unwrap();
        let claim = cctp_payout_claim(
            vec![],
            U256::from(5_000_000),
            U256::from(6_000_000),
            &fee(),
            destination,
            Address::repeat_byte(0x11),
            Address::repeat_byte(0x22),
        )
        .unwrap();
        let OutboundBridge::Cctp { burn, .. } = claim.bridge else {
            panic!("expected a CCTP bridge")
        };
        assert_eq!(burn.amount, U256::from(6_000_000));
        assert_eq!(burn.destination_domain, 7);
    }

    #[test]
    fn plan_refuses_a_floor_the_fee_cap_consumes_and_bad_destinations() {
        let ethereum = cctp::CctpChain::for_bridge_name("Ethereum").unwrap();
        // About $1.16 of forwarding fee against a $1 floor.
        let error = cctp_payout_claim(
            vec![],
            U256::from(1_000_000),
            U256::from(1_000_000),
            &fee(),
            ethereum,
            Address::repeat_byte(0x11),
            Address::repeat_byte(0x22),
        )
        .unwrap_err();
        assert!(error.to_string().contains("too small to bridge"), "{error}");
        let arbitrum = cctp::CctpChain::for_chain(42161).unwrap();
        for (destination, recipient) in [
            (arbitrum, Address::repeat_byte(0x11)),
            (ethereum, Address::ZERO),
        ] {
            assert!(
                cctp_payout_claim(
                    vec![],
                    U256::from(10_000_000),
                    U256::from(9_000_000),
                    &fee(),
                    destination,
                    recipient,
                    Address::repeat_byte(0x22),
                )
                .is_err()
            );
        }
    }
}
