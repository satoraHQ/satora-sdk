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
    /// Confirmations the funding needs before it is claimed.
    pub minimum_confirmations: u32,
    pub minimum_remaining_blocks: u32,
    pub confirmation_target: u16,
}
impl BitcoinAutoClaim {
    pub fn new(backend: Arc<dyn BitcoinBackend>, maximum_fee: Amount) -> Self {
        Self {
            backend,
            maximum_fee,
            minimum_confirmations: 1,
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
}
#[cfg(feature = "gasless")]
impl Default for EvmAutoClaim {
    fn default() -> Self {
        Self {
            chains: Default::default(),
            slippage_bps: None,
            minimum_remaining_seconds: 300,
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
            EvmToLightning(_) | ArkadeToLightning(_) => {
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
            EvmToBitcoin(_) => self.claim_bitcoin(id, destination).await.map(Some),
            BitcoinToEvm(_) | ArkadeToEvm(_) | LightningToEvm(_) => {
                self.claim_evm(id, stored, &destination).await
            }
            EvmToLightning(_) | ArkadeToLightning(_) => {
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

    async fn claim_bitcoin(&self, id: &str, destination: String) -> Result<String> {
        let config = self
            .config
            .bitcoin
            .as_ref()
            .ok_or_else(|| invalid("automatic Bitcoin claims are not configured"))?;
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
                    minimum_confirmations: config.minimum_confirmations,
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
        // The daemon builds bridge calls with no minimum output.
        if !value["bridge_target_chain"].is_null() {
            return Err(invalid(
                "bridged payouts are not claimed automatically; claim with a reviewed bridge plan",
            ));
        }
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
                        tracing::warn!(%id, %error, "sponsored claim sent nothing; claiming through the server relay");
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
