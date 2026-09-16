//! Circle CCTP v2. Registry mirrors the TypeScript SDK; deployments are explicit
//! configuration so test networks never accidentally use mainnet contracts.
use super::BridgeApi;
use super::invalid;
use super::pending_json;
use crate::Result;
use crate::control::PollOptions;
use alloy::primitives::Address;
use alloy::primitives::B256;
use alloy::primitives::Bytes;
use alloy::primitives::U256;
use alloy::primitives::address;
use alloy::sol;
use alloy::sol_types::SolCall;
use serde::Deserialize;
use serde::Serialize;

pub const TOKEN_MESSENGER: Address = address!("28b5a0e9C621a5BadaA536219b3a228C8168cf5d");
pub const MESSAGE_TRANSMITTER: Address = address!("81D40F21F12A8F0E3252Bccb954D722d4c464B64");
pub const ARBITRUM_ADAPTER: Address = address!("cdf4104ade1adeaa47a2327b836a7b099a90a67f");
pub const SOLANA_DOMAIN: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CctpChain {
    pub chain_id: u64,
    pub domain: u32,
    pub usdc: Address,
}
impl CctpChain {
    pub fn for_chain(chain_id: u64) -> Result<Self> {
        CHAINS
            .iter()
            .copied()
            .find(|chain| chain.chain_id == chain_id)
            .ok_or_else(|| invalid("unsupported CCTP EVM chain"))
    }
    pub fn finality(self) -> u32 {
        if self.chain_id == 50 { 2000 } else { 1000 }
    }
    pub fn requires_bridge(self) -> bool {
        !matches!(self.chain_id, 1 | 137 | 42161)
    }
}

sol! {
    function depositForBurn(uint256 amount, uint32 destinationDomain, bytes32 mintRecipient, address burnToken, bytes32 destinationCaller, uint256 maxFee, uint32 minFinalityThreshold) external;
    function depositForBurnWithHook(uint256 amount, uint32 destinationDomain, bytes32 mintRecipient, address burnToken, bytes32 destinationCaller, uint256 maxFee, uint32 minFinalityThreshold, bytes hookData) external;
    function receiveMessage(bytes message, bytes attestation) external returns (bool);
    function usedNonces(bytes32 nonce) external view returns (uint256);
    event MessageSent(bytes message);
    event MessageReceived(address indexed caller, uint32 sourceDomain, bytes32 indexed nonce, bytes32 sender, uint32 indexed finalityThresholdExecuted, bytes messageBody);
    function bridgeBalance(uint32 destinationDomain, bytes32 mintRecipient, uint256 maxFee, bytes hookData) external;
}

pub fn evm_recipient(address: Address) -> B256 {
    address.into_word()
}
/// Solana ATA/public-key bytes, already decoded by the caller from base58.
pub fn solana_recipient(public_key: [u8; 32]) -> B256 {
    public_key.into()
}
pub fn forwarding_hook(setup_solana_wallet: Option<[u8; 32]>) -> Bytes {
    let mut hook = vec![0; 32];
    hook[..12].copy_from_slice(b"cctp-forward");
    if let Some(wallet) = setup_solana_wallet {
        hook[31] = 33;
        hook.push(1);
        hook.extend(wallet);
    }
    hook.into()
}

/// All immutable fields expected from a burn. Never choose an attestation just
/// because it is the first message returned for a transaction.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CctpBurn {
    pub source_chain_id: u64,
    pub source_domain: u32,
    pub destination_domain: u32,
    pub source_messenger: Address,
    pub destination_messenger: B256,
    pub source_transmitter: Address,
    pub burn_token: Address,
    pub sender: Address,
    pub mint_recipient: B256,
    pub destination_caller: B256,
    pub amount: U256,
    pub max_fee: U256,
    pub min_finality: u32,
    pub hook: Bytes,
}
impl CctpBurn {
    pub fn validate(&self) -> Result<()> {
        if self.source_chain_id == 0
            || self.source_domain == self.destination_domain
            || self.source_messenger.is_zero()
            || self.source_transmitter.is_zero()
            || self.destination_messenger.is_zero()
            || self.burn_token.is_zero()
            || self.sender.is_zero()
            || self.mint_recipient.is_zero()
            || self.amount.is_zero()
            || self.max_fee >= self.amount
            || !matches!(self.min_finality, 1000 | 2000)
        {
            return Err(invalid("invalid CCTP burn parameters"));
        }
        Ok(())
    }
    pub fn calldata(&self) -> Result<Bytes> {
        self.validate()?;
        Ok(if self.hook.is_empty() {
            depositForBurnCall {
                amount: self.amount,
                destinationDomain: self.destination_domain,
                mintRecipient: self.mint_recipient,
                burnToken: self.burn_token,
                destinationCaller: self.destination_caller,
                maxFee: self.max_fee,
                minFinalityThreshold: self.min_finality,
            }
            .abi_encode()
        } else {
            depositForBurnWithHookCall {
                amount: self.amount,
                destinationDomain: self.destination_domain,
                mintRecipient: self.mint_recipient,
                burnToken: self.burn_token,
                destinationCaller: self.destination_caller,
                maxFee: self.max_fee,
                minFinalityThreshold: self.min_finality,
                hookData: self.hook.clone(),
            }
            .abi_encode()
        }
        .into())
    }
    pub fn validate_message(&self, bytes: &[u8]) -> Result<CctpMessage> {
        self.validate()?;
        let message = CctpMessage::decode(bytes)?;
        if message.source_domain != self.source_domain
            || message.destination_domain != self.destination_domain
            || message.sender != self.source_messenger.into_word()
            || message.recipient != self.destination_messenger
            || message.destination_caller != self.destination_caller
            || message.min_finality != self.min_finality
            || message.burn_token != self.burn_token.into_word()
            || message.mint_recipient != self.mint_recipient
            || message.amount != self.amount
            || message.message_sender != self.sender.into_word()
            || message.max_fee != self.max_fee
            || message.hook != self.hook
        {
            return Err(invalid("CCTP message does not match the approved burn"));
        }
        Ok(message)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CctpMessage {
    pub source_domain: u32,
    pub destination_domain: u32,
    pub nonce: B256,
    pub sender: B256,
    pub recipient: B256,
    pub destination_caller: B256,
    pub min_finality: u32,
    pub executed_finality: u32,
    pub burn_token: B256,
    pub mint_recipient: B256,
    pub amount: U256,
    pub message_sender: B256,
    pub max_fee: U256,
    pub fee_executed: U256,
    pub expiration_block: U256,
    pub hook: Bytes,
}
impl CctpMessage {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 376 {
            return Err(invalid("truncated CCTP v2 message"));
        }
        let u32_at =
            |i| u32::from_be_bytes(bytes[i..i + 4].try_into().expect("checked message size"));
        let word = |i| B256::from_slice(&bytes[i..i + 32]);
        let uint = |i| U256::from_be_slice(&bytes[i..i + 32]);
        if u32_at(0) != 1 || u32_at(148) != 1 {
            return Err(invalid("unsupported CCTP message version"));
        }
        let message = Self {
            source_domain: u32_at(4),
            destination_domain: u32_at(8),
            nonce: word(12),
            sender: word(44),
            recipient: word(76),
            destination_caller: word(108),
            min_finality: u32_at(140),
            executed_finality: u32_at(144),
            burn_token: word(152),
            mint_recipient: word(184),
            amount: uint(216),
            message_sender: word(248),
            max_fee: uint(280),
            fee_executed: uint(312),
            expiration_block: uint(344),
            hook: bytes[376..].to_vec().into(),
        };
        if message.fee_executed > message.max_fee || message.fee_executed >= message.amount {
            return Err(invalid("CCTP executed fee exceeds the approved amount"));
        }
        Ok(message)
    }
    pub fn minted_amount(&self) -> U256 {
        self.amount - self.fee_executed
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CctpAttestation {
    pub message: String,
    pub attestation: String,
    pub status: String,
    pub event_nonce: Option<String>,
    pub forward_state: Option<String>,
    pub forward_tx_hash: Option<String>,
}
impl CctpAttestation {
    pub fn receive_calldata(&self, burn: &CctpBurn) -> Result<Bytes> {
        let message: Bytes = self
            .message
            .parse()
            .map_err(|_| invalid("invalid CCTP message hex"))?;
        let attestation: Bytes = self
            .attestation
            .parse()
            .map_err(|_| invalid("invalid CCTP attestation hex"))?;
        let decoded = burn.validate_message(&message)?;
        if self.status != "complete"
            || attestation.is_empty()
            || decoded.nonce.is_zero()
            || decoded.executed_finality < burn.min_finality
        {
            return Err(invalid("CCTP attestation is incomplete"));
        }
        Ok(receiveMessageCall {
            message,
            attestation,
        }
        .abi_encode()
        .into())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CctpFee {
    pub finality_threshold: u32,
    pub minimum_fee: serde_json::Number,
    pub forward_fee: Option<ForwardFee>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct ForwardFee {
    pub high: u64,
}
impl CctpFee {
    /// Integer-only basis-point arithmetic, rounded to hundredths as in the TS SDK.
    pub fn max_fee(&self, amount: U256, forward: bool) -> Result<U256> {
        let text = self.minimum_fee.to_string();
        let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
        let whole = whole
            .parse::<u64>()
            .map_err(|_| invalid("invalid CCTP fee rate"))?;
        if !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid("invalid fee rate"));
        }
        let mut digits = fraction.bytes().chain(std::iter::repeat(b'0'));
        let scaled = U256::from(whole) * U256::from(100)
            + U256::from(
                (digits.next().unwrap() - b'0') as u64 * 10
                    + (digits.next().unwrap() - b'0') as u64,
            )
            + U256::from(u64::from(digits.next().unwrap() >= b'5'));
        let fee = amount
            .checked_mul(scaled)
            .ok_or_else(|| invalid("fee overflow"))?
            / U256::from(1_000_000);
        let fee = fee
            .checked_mul(U256::from(120))
            .ok_or_else(|| invalid("fee overflow"))?
            / U256::from(100);
        fee.checked_add(if forward {
            U256::from(
                self.forward_fee
                    .as_ref()
                    .ok_or_else(|| invalid("forward fee unavailable"))?
                    .high,
            )
        } else {
            U256::ZERO
        })
        .ok_or_else(|| invalid("fee overflow"))
    }
}

impl BridgeApi {
    pub async fn cctp_fee(&self, source: u32, destination: u32, finality: u32) -> Result<CctpFee> {
        let url = self.iris.join(&format!(
            "/v2/burn/USDC/fees/{source}/{destination}?forward=true"
        ))?;
        let response = crate::client::check_status(self.http.get(url).send().await?).await?;
        let entries: Vec<CctpFee> = response.json().await?;
        entries
            .into_iter()
            .find(|fee| fee.finality_threshold == finality)
            .ok_or_else(|| invalid("requested CCTP finality fee is unavailable"))
    }
    pub async fn cctp_attestation(
        &self,
        txid: B256,
        burn: &CctpBurn,
    ) -> Result<Option<CctpAttestation>> {
        #[derive(Deserialize)]
        struct Response {
            messages: Vec<CctpAttestation>,
        }
        let url = self
            .iris
            .join(&format!("/v2/messages/{}", burn.source_domain))?;
        let Some(response) = pending_json::<Response>(
            self.http
                .get(url)
                .query(&[("transactionHash", txid.to_string())])
                .send()
                .await?,
        )
        .await?
        else {
            return Ok(None);
        };
        let mut matches = Vec::new();
        for entry in response.messages {
            if entry.status != "complete" {
                continue;
            }
            let bytes: Bytes = entry
                .message
                .parse()
                .map_err(|_| invalid("invalid attested message hex"))?;
            if burn.validate_message(&bytes).is_ok() {
                entry.receive_calldata(burn)?;
                matches.push(entry);
            }
        }
        if matches.len() > 1 {
            return Err(invalid("ambiguous CCTP messages for approved burn"));
        }
        Ok(matches.pop())
    }
    pub async fn wait_cctp_attestation(
        &self,
        txid: B256,
        burn: &CctpBurn,
        options: &PollOptions,
    ) -> Result<CctpAttestation> {
        options
            .run(async {
                loop {
                    match self.cctp_attestation(txid, burn).await {
                        Ok(Some(attestation)) => return Ok(attestation),
                        Ok(None) | Err(crate::Error::Transport(_)) => {}
                        Err(error) => return Err(error),
                    }
                    tokio::time::sleep(options.interval).await;
                }
            })
            .await
    }
    /// Wait for Circle's forwarding service, including EVM and Solana recipients.
    /// A failed forward retains a valid attestation for explicit mint recovery.
    pub async fn wait_cctp_forward(
        &self,
        txid: B256,
        burn: &CctpBurn,
        options: &PollOptions,
    ) -> Result<String> {
        if burn.hook.is_empty() {
            return Err(invalid("burn did not request forwarding"));
        }
        options
            .run(async {
                loop {
                    match self.cctp_attestation(txid, burn).await {
                        Ok(Some(attestation)) => match attestation.forward_state.as_deref() {
                            Some("COMPLETE" | "CONFIRMED") => {
                                return attestation
                                    .forward_tx_hash
                                    .filter(|hash| !hash.is_empty())
                                    .ok_or_else(|| {
                                        invalid("forwarded message has no destination transaction")
                                    });
                            }
                            None | Some("PENDING") => {}
                            Some(state) => {
                                return Err(invalid(format!(
                                    "CCTP forwarding requires recovery: {state}"
                                )));
                            }
                        },
                        Ok(None) | Err(crate::Error::Transport(_)) => {}
                        Err(error) => return Err(error),
                    }
                    tokio::time::sleep(options.interval).await;
                }
            })
            .await
    }

    /// Explicit retry after a destination contract reports expiry. Circle uses
    /// L1 heights on some rollups; do not compare expiration to an L2 height.
    pub async fn reattest_cctp(&self, nonce: B256) -> Result<()> {
        let url = self.iris.join(&format!("/v2/reattest/{nonce}"))?;
        crate::client::check_status(self.http.post(url).send().await?).await?;
        Ok(())
    }
}

pub const CHAINS: &[CctpChain] = &[
    CctpChain {
        chain_id: 1,
        domain: 0,
        usdc: address!("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"),
    },
    CctpChain {
        chain_id: 10,
        domain: 2,
        usdc: address!("0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85"),
    },
    CctpChain {
        chain_id: 130,
        domain: 10,
        usdc: address!("0x078d782b760474a361dda0af3839290b0ef57ad6"),
    },
    CctpChain {
        chain_id: 137,
        domain: 7,
        usdc: address!("0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359"),
    },
    CctpChain {
        chain_id: 146,
        domain: 13,
        usdc: address!("0x29219dd400f2bf60e5a23d13be72b486d4038894"),
    },
    CctpChain {
        chain_id: 1329,
        domain: 16,
        usdc: address!("0xe15fC38F6D8c56aF07bbCBe3BAf5708A2Bf42392"),
    },
    CctpChain {
        chain_id: 8453,
        domain: 6,
        usdc: address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
    },
    CctpChain {
        chain_id: 42161,
        domain: 3,
        usdc: address!("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
    },
    CctpChain {
        chain_id: 43114,
        domain: 1,
        usdc: address!("0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E"),
    },
    CctpChain {
        chain_id: 59144,
        domain: 11,
        usdc: address!("0x176211869ca2b568f2a7d4ee941e073a821ee1ff"),
    },
    CctpChain {
        chain_id: 480,
        domain: 14,
        usdc: address!("0x79A02482A880bCe3F13E09da970dC34dB4cD24D1"),
    },
    CctpChain {
        chain_id: 57073,
        domain: 21,
        usdc: address!("0x2D270e6886d130D724215A266106e6832161EAEd"),
    },
    CctpChain {
        chain_id: 999,
        domain: 19,
        usdc: address!("0xb88339CB7199b77E23DB6E890353E22632Ba630f"),
    },
    CctpChain {
        chain_id: 143,
        domain: 15,
        usdc: address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603"),
    },
    CctpChain {
        chain_id: 50,
        domain: 18,
        usdc: address!("0xfA2958CB79b0491CC627c1557F441eF849Ca8eb1"),
    },
];
