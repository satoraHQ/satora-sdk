use super::*;
use crate::Client;
use crate::types::GetSwapResponse;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwapContracts {
    /// None for Lightning payments made by the client.
    pub client: Option<ContractRef>,
    /// None for Lightning payments received by the client.
    pub server: Option<ContractRef>,
}
#[derive(Clone, Debug)]
pub struct ReferenceOptions {
    pub client_confirmations: u32,
    pub server_confirmations: u32,
    #[cfg(feature = "gasless")]
    pub evm_from_block: u64,
    /// Required when the source response has no coordinator/sender identity.
    #[cfg(feature = "gasless")]
    pub evm_source_sender: Option<crate::evm::Address>,
    /// Required when an EVM→Arkade response omits the locked BTC token address.
    #[cfg(feature = "gasless")]
    pub evm_lock_token: Option<crate::evm::Address>,
}
impl Default for ReferenceOptions {
    fn default() -> Self {
        Self {
            client_confirmations: 1,
            server_confirmations: 1,
            #[cfg(feature = "gasless")]
            evm_from_block: 0,
            #[cfg(feature = "gasless")]
            evm_source_sender: None,
            #[cfg(feature = "gasless")]
            evm_lock_token: None,
        }
    }
}
impl Client {
    /// Reconstruct references from a stored swap, validating Bitcoin/Arkade
    /// addresses against their scripts and seed. Lightning's remote invoice leg
    /// is deliberately absent: API status cannot prove its settlement.
    pub fn contract_references(
        &self,
        id: &str,
        options: &ReferenceOptions,
    ) -> Result<SwapContracts> {
        use GetSwapResponse::*;
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        let value =
            serde_json::to_value(&stored.response).map_err(|e| Error::Decode(e.to_string()))?;
        let bitcoin = |client: bool| -> Result<ContractRef> {
            let amount = value[if client {
                "source_amount"
            } else {
                "target_amount"
            }]
            .as_str()
            .ok_or_else(|| invalid("Bitcoin amount missing"))?
            .parse::<u64>()
            .map_err(|e| Error::Decode(e.to_string()))?;
            if amount == 0 || amount > bitcoin::Amount::MAX_MONEY.to_sat() {
                return Err(invalid("invalid expected Bitcoin amount"));
            }
            Ok(ContractRef::Bitcoin {
                htlc: self.bitcoin_htlc(id)?,
                expected_sats: amount,
                confirmations: if client {
                    options.client_confirmations
                } else {
                    options.server_confirmations
                },
            })
        };
        let arkade = || -> Result<ContractRef> {
            let h = self.arkade_htlc(id)?;
            Ok(ContractRef::Arkade {
                options: h.options,
                network: h.network,
                expected_sats: h.expected_amount.to_sat(),
            })
        };
        #[cfg(feature = "gasless")]
        let evm = |client: bool| -> Result<ContractRef> {
            use crate::evm::Address;
            use crate::evm::EvmContext;
            use crate::evm::decimal;
            use crate::evm::htlc::EvmHtlc;
            use crate::evm::parse;
            let context = EvmContext::from_response(&stored.response)?;
            let field = |name: &str| -> Result<&str> {
                value[name]
                    .as_str()
                    .ok_or_else(|| invalid(format!("missing EVM {name}")))
            };
            let token = match context.kind {
                crate::types::EvmHtlcKind::Native => Address::ZERO,
                crate::types::EvmHtlcKind::Erc20 => options
                    .evm_lock_token
                    .or_else(|| value["wbtc_address"].as_str().and_then(|s| s.parse().ok()))
                    .ok_or_else(|| {
                        invalid("configure the locked EVM token; the swap response omits it")
                    })?,
            };
            let sender = if client {
                options
                    .evm_source_sender
                    .or_else(|| {
                        stored
                            .metadata
                            .evm_coordinator_address
                            .as_deref()
                            .and_then(|s| s.parse().ok())
                    })
                    .or_else(|| {
                        value["evm_coordinator_address"]
                            .as_str()
                            .and_then(|s| s.parse().ok())
                    })
                    .ok_or_else(|| {
                        invalid("configure the EVM source sender/coordinator before tracking")
                    })?
            } else {
                context.server
            };
            if (token == Address::ZERO && context.kind == crate::types::EvmHtlcKind::Erc20)
                || sender == Address::ZERO
            {
                return Err(invalid("invalid EVM reference token or sender"));
            }
            let amount = decimal(field("evm_expected_sats")?)?;
            if amount.is_zero() {
                return Err(invalid("expected EVM funding must be positive"));
            }
            let hash = if let Some(hash) = value["evm_hash_lock"].as_str() {
                parse(hash)?
            } else {
                parse(field("hash_lock")?)?
            };
            Ok(ContractRef::Evm {
                htlc: EvmHtlc {
                    chain_id: context.chain_id,
                    address: context.htlc,
                    hash_lock: hash,
                    amount,
                    token,
                    sender,
                    claimer: if client {
                        context.server
                    } else {
                        context.owner
                    },
                    timelock: context.timelock,
                },
                from_block: options.evm_from_block,
                confirmations: if client {
                    options.client_confirmations
                } else {
                    options.server_confirmations
                },
            })
        };
        #[cfg(not(feature = "gasless"))]
        let evm = |_: bool| -> Result<ContractRef> {
            Err(invalid("EVM observations require the gasless feature"))
        };
        Ok(match &stored.response {
            BtcToArkade(_) => SwapContracts {
                client: Some(bitcoin(true)?),
                server: Some(arkade()?),
            },
            BitcoinToEvm(_) => SwapContracts {
                client: Some(bitcoin(true)?),
                server: Some(evm(false)?),
            },
            EvmToBitcoin(_) => SwapContracts {
                client: Some(evm(true)?),
                server: Some(bitcoin(false)?),
            },
            EvmToArkade(_) => SwapContracts {
                client: Some(evm(true)?),
                server: Some(arkade()?),
            },
            ArkadeToEvm(_) => SwapContracts {
                client: Some(arkade()?),
                server: Some(evm(false)?),
            },
            ArkadeToLightning(_) => SwapContracts {
                client: Some(arkade()?),
                server: None,
            },
            LightningToArkade(_) => SwapContracts {
                client: None,
                server: Some(arkade()?),
            },
            EvmToLightning(_) => SwapContracts {
                client: Some(evm(true)?),
                server: None,
            },
            LightningToEvm(_) => SwapContracts {
                client: None,
                server: Some(evm(false)?),
            },
        })
    }
}

/// Dispatch one manager across independently configured Bitcoin, Arkade and EVM readers.
pub struct ReaderSet(pub Vec<std::sync::Arc<dyn ContractReader>>);
impl ContractReader for ReaderSet {
    fn can_observe(&self, reference: &ContractRef) -> bool {
        self.0.iter().any(|reader| reader.can_observe(reference))
    }
    fn observe<'a>(&'a self, reference: &'a ContractRef) -> ObservationFuture<'a, Observation> {
        Box::pin(async move {
            let mut readers = self.0.iter().filter(|reader| reader.can_observe(reference));
            let reader = readers
                .next()
                .ok_or_else(|| invalid("no reader configured for contract"))?;
            if readers.next().is_some() {
                return Err(invalid("multiple readers configured for the same contract"));
            }
            reader.observe(reference).await
        })
    }
}
