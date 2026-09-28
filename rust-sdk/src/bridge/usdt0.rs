//! USDT0 OFT encoding and LayerZero delivery tracking.
use super::BridgeApi;
use super::invalid;
use crate::Result;
use crate::control::PollOptions;
use crate::evm::EvmReceipt;
use crate::evm::EvmWallet;
use crate::evm::funding::transaction;
use alloy::primitives::Address;
use alloy::primitives::B256;
use alloy::primitives::Bytes;
use alloy::primitives::U256;
use alloy::primitives::address;
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolEvent;
use serde::Deserialize;
use serde::Serialize;

pub const ARBITRUM_OFT: Address = address!("14E4A1B13bf7F943c8ff7C51fb60FA964A298D92");
pub const ARBITRUM_ADAPTER: Address = address!("a950e7c1aC2C524e42291D5883C696Ac332D23a0");

sol! {
    #[derive(Debug)]
    struct SendParam { uint32 dstEid; bytes32 to; uint256 amountLD; uint256 minAmountLD; bytes extraOptions; bytes composeMsg; bytes oftCmd; }
    #[derive(Debug)]
    struct MessagingFee { uint256 nativeFee; uint256 lzTokenFee; }
    function quoteSend(SendParam param, bool payInLzToken) external view returns (MessagingFee fee);
    function send(SendParam param, MessagingFee fee, address refundAddress) external payable;
    function bridgeBalance(uint32 dstEid, bytes32 to) external payable;
    event OFTSent(bytes32 indexed guid, uint32 dstEid, address indexed fromAddress, uint256 amountSentLD, uint256 amountReceivedLD);
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OftTransfer {
    pub chain_id: u64,
    pub source_eid: u32,
    pub destination_eid: u32,
    pub token: Address,
    /// Pin the OFT contract separately: it is not always the ERC20 token.
    pub oft: Address,
    pub sender: Address,
    pub recipient: B256,
    pub amount: U256,
    pub minimum_received: U256,
    pub max_native_fee: U256,
}
impl OftTransfer {
    pub fn params(&self) -> Result<SendParam> {
        if self.chain_id == 0
            || self.source_eid == self.destination_eid
            || self.source_eid == 0
            || self.destination_eid == 0
            || self.token.is_zero()
            || self.oft.is_zero()
            || self.sender.is_zero()
            || self.recipient.is_zero()
            || self.amount.is_zero()
            || self.minimum_received.is_zero()
            || self.minimum_received > self.amount
        {
            return Err(invalid("invalid OFT transfer parameters"));
        }
        Ok(SendParam {
            dstEid: self.destination_eid,
            to: self.recipient,
            amountLD: self.amount,
            minAmountLD: self.minimum_received,
            extraOptions: Bytes::new(),
            composeMsg: Bytes::new(),
            oftCmd: Bytes::new(),
        })
    }
    /// Fresh on-chain quote; no guessed fallback fee. Bounds the payable amount.
    pub async fn quote(&self, wallet: &dyn EvmWallet) -> Result<U256> {
        if wallet.chain_id() != self.chain_id || wallet.address() != self.sender {
            return Err(invalid("OFT wallet does not match transfer"));
        }
        let response = wallet
            .call(&transaction(
                wallet,
                self.oft,
                quoteSendCall {
                    param: self.params()?,
                    payInLzToken: false,
                }
                .abi_encode(),
            ))
            .await?;
        let fee = quoteSendCall::abi_decode_returns(&response)
            .map_err(|_| invalid("invalid OFT fee response"))?;
        if !fee.lzTokenFee.is_zero() {
            return Err(invalid("OFT requires an unsupported LZ token fee"));
        }
        let buffered = fee
            .nativeFee
            .checked_add(fee.nativeFee / U256::from(5))
            .ok_or_else(|| invalid("native fee overflow"))?;
        if buffered > self.max_native_fee {
            return Err(invalid("OFT native fee exceeds approved limit"));
        }
        Ok(buffered)
    }
    pub fn calldata(&self, fee: U256) -> Result<Bytes> {
        if fee > self.max_native_fee {
            return Err(invalid("OFT native fee exceeds approved limit"));
        }
        Ok(sendCall {
            param: self.params()?,
            fee: MessagingFee {
                nativeFee: fee,
                lzTokenFee: U256::ZERO,
            },
            refundAddress: self.sender,
        }
        .abi_encode()
        .into())
    }
}

/// OFTSent events emitted by `oft`. Other logs are skipped.
pub(super) fn oft_sent(receipt: &EvmReceipt, oft: Address) -> impl Iterator<Item = OFTSent> + '_ {
    receipt
        .logs
        .iter()
        .filter(move |log| log.address == oft)
        .filter_map(|log| {
            OFTSent::decode_raw_log_validate(log.topics.iter().copied(), &log.data).ok()
        })
}

/// The contract that emits OFTSent and the address the send debits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OftRoute {
    pub oft: Address,
    pub from: Address,
    pub source_eid: u32,
    pub destination_eid: u32,
}
impl OftRoute {
    /// The adapter route a coordinator claim into USDT0 uses.
    pub fn arbitrum_adapter(destination_eid: u32) -> Self {
        Self {
            oft: ARBITRUM_OFT,
            from: ARBITRUM_ADAPTER,
            source_eid: 30110,
            destination_eid,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeliveryStatus {
    Inflight,
    Confirming,
    Delivered,
    Failed,
    Blocked,
    /// Verified on the destination, but execution reverted. It is delivered only
    /// after someone retries `lzReceive` on the destination endpoint.
    PayloadStored,
    ApplicationBurned,
    ApplicationSkipped,
    UnresolvableCommand,
    MalformedCommand,
    /// A status LayerZero Scan added after this SDK; treated as pending.
    #[serde(other)]
    Unknown,
}
impl DeliveryStatus {
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Inflight | Self::Confirming | Self::Unknown)
    }
    /// Waiting alone will not deliver the message.
    pub fn needs_intervention(&self) -> bool {
        matches!(
            self,
            Self::Failed
                | Self::Blocked
                | Self::PayloadStored
                | Self::ApplicationBurned
                | Self::ApplicationSkipped
                | Self::UnresolvableCommand
                | Self::MalformedCommand
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub guid: B256,
    pub status: DeliveryStatus,
    pub source_txid: B256,
    /// Can be missing even once delivered.
    pub destination_txid: Option<B256>,
    pub source_eid: u32,
    pub destination_eid: u32,
    /// LayerZero Scan's chain names (e.g. "arbitrum"), not the daemon's.
    pub source_chain: Option<String>,
    pub destination_chain: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageQuery {
    pub source_txid: B256,
    pub source_eid: u32,
    pub destination_eid: u32,
    /// Obtain from the source receipt's OFTSent event. Required to distinguish
    /// multiple transfers in a single transaction.
    pub guid: B256,
}
impl MessageQuery {
    /// Identifies the one OFT send on `route` in a successful receipt, for a claim
    /// that returned only its transaction hash (such as `claim_evm_gasless`).
    pub fn from_receipt(receipt: &EvmReceipt, route: &OftRoute) -> Result<Self> {
        if !receipt.success {
            return Err(invalid("OFT send transaction failed"));
        }
        let guids: Vec<B256> = oft_sent(receipt, route.oft)
            .filter(|event| {
                event.dstEid == route.destination_eid
                    && event.fromAddress == route.from
                    && !event.guid.is_zero()
            })
            .map(|event| event.guid)
            .collect();
        let [guid] = guids[..] else {
            return Err(invalid(
                "receipt must contain exactly one OFT send on the route",
            ));
        };
        Ok(Self {
            source_txid: receipt.transaction_hash,
            source_eid: route.source_eid,
            destination_eid: route.destination_eid,
            guid,
        })
    }
}
/// A lookup by source transaction. Every field that is set must match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageLookup {
    pub source_txid: B256,
    pub guid: Option<B256>,
    pub source_eid: Option<u32>,
    pub destination_eid: Option<u32>,
}
impl MessageLookup {
    pub fn tx(source_txid: B256) -> Self {
        Self {
            source_txid,
            ..Self::default()
        }
    }
}
impl From<&MessageQuery> for MessageLookup {
    fn from(query: &MessageQuery) -> Self {
        Self {
            source_txid: query.source_txid,
            guid: Some(query.guid),
            source_eid: Some(query.source_eid),
            destination_eid: Some(query.destination_eid),
        }
    }
}

#[doc(alias = "getLzExplorerUrl")]
pub fn explorer_url(source_txid: B256) -> String {
    format!("https://layerzeroscan.com/tx/{source_txid}")
}

impl BridgeApi {
    pub async fn usdt0_status(&self, query: &MessageQuery) -> Result<Option<Delivery>> {
        self.usdt0_status_by_tx(&query.into()).await
    }
    /// `Ok(None)` until LayerZero Scan indexes the message. Without a GUID, several
    /// matching messages are an error rather than a guess. Scan may have indexed
    /// only some of them yet, so pass the GUID from [`MessageQuery::from_receipt`]
    /// when the transaction can carry more than one send.
    #[doc(alias = "getLzMessageStatus")]
    pub async fn usdt0_status_by_tx(&self, lookup: &MessageLookup) -> Result<Option<Delivery>> {
        #[derive(Deserialize)]
        struct Response {
            data: Vec<Message>,
        }
        #[derive(Deserialize)]
        struct Message {
            guid: B256,
            status: Status,
            pathway: Pathway,
            #[serde(default)]
            source: Side,
            #[serde(default)]
            destination: Side,
        }
        #[derive(Deserialize)]
        struct Status {
            name: DeliveryStatus,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Pathway {
            src_eid: u32,
            dst_eid: u32,
            #[serde(default)]
            sender: Party,
            #[serde(default)]
            receiver: Party,
        }
        #[derive(Default, Deserialize)]
        struct Party {
            chain: Option<String>,
        }
        #[derive(Default, Deserialize)]
        struct Side {
            tx: Option<Tx>,
        }
        // A string, not B256: non-EVM chains (Solana) use other hash formats.
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Tx {
            tx_hash: String,
        }
        fn hash(side: &Side) -> Option<B256> {
            side.tx.as_ref()?.tx_hash.parse().ok()
        }
        let txid = lookup.source_txid.to_string();
        let url = super::endpoint(&self.layerzero, &["v1", "messages", "tx", &txid])?;
        let response = self.http.get(url).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response: Response = crate::client::check_status(response).await?.json().await?;
        // The endpoint also lists messages this transaction delivered on a destination.
        let identity = |m: &Message| {
            hash(&m.source) == Some(lookup.source_txid)
                && lookup.source_eid.is_none_or(|eid| eid == m.pathway.src_eid)
                && lookup
                    .destination_eid
                    .is_none_or(|eid| eid == m.pathway.dst_eid)
        };
        let mut matching: Vec<Message> = response
            .data
            .into_iter()
            .filter(|m| {
                lookup
                    .guid
                    .map_or_else(|| identity(m), |guid| m.guid == guid)
            })
            .collect();
        if matching.len() > 1 {
            return Err(invalid(match lookup.guid {
                Some(_) => "LayerZero message identity mismatch".to_string(),
                None => format!(
                    "{} LayerZero messages match {txid}; pass the GUID (MessageQuery::from_receipt) or the destination EID",
                    matching.len()
                ),
            }));
        }
        let Some(message) = matching.pop() else {
            return Ok(None);
        };
        if !identity(&message) {
            return Err(invalid("LayerZero message identity mismatch"));
        }
        Ok(Some(Delivery {
            guid: message.guid,
            status: message.status.name,
            source_txid: lookup.source_txid,
            destination_txid: hash(&message.destination),
            source_eid: message.pathway.src_eid,
            destination_eid: message.pathway.dst_eid,
            source_chain: message.pathway.sender.chain,
            destination_chain: message.pathway.receiver.chain,
        }))
    }
    pub async fn wait_usdt0(
        &self,
        query: &MessageQuery,
        options: &PollOptions,
    ) -> Result<Delivery> {
        self.wait_usdt0_by_tx(&query.into(), options, |_| {}).await
    }
    /// Polls until the message is delivered. `on_status` sees each status change.
    /// Read errors are retried until the deadline; a status that waiting cannot
    /// resolve ([`DeliveryStatus::needs_intervention`]) stops the wait.
    #[doc(alias = "trackLzMessage")]
    pub async fn wait_usdt0_by_tx(
        &self,
        lookup: &MessageLookup,
        options: &PollOptions,
        mut on_status: impl FnMut(&Delivery) + Send,
    ) -> Result<Delivery> {
        let mut last_status = None;
        let mut last_error = None;
        let result = options
            .run(async {
                loop {
                    match self.usdt0_status_by_tx(lookup).await {
                        Ok(Some(delivery)) => {
                            if last_status != Some(delivery.status) {
                                last_status = Some(delivery.status);
                                on_status(&delivery);
                            }
                            if delivery.status == DeliveryStatus::Delivered {
                                return Ok(delivery);
                            }
                            if delivery.status.needs_intervention() {
                                let retry = if delivery.status == DeliveryStatus::PayloadStored {
                                    "; anyone can retry lzReceive on the destination endpoint"
                                } else {
                                    ""
                                };
                                return Err(invalid(format!(
                                    "LayerZero message {} is {:?}{retry}: {}",
                                    delivery.guid,
                                    delivery.status,
                                    explorer_url(lookup.source_txid)
                                )));
                            }
                        }
                        Ok(None) => {}
                        Err(
                            error @ (crate::Error::Transport(_)
                            | crate::Error::Api { .. }
                            | crate::Error::Decode(_)),
                        ) => last_error = Some(error.to_string()),
                        Err(error) => return Err(error),
                    }
                    tokio::time::sleep(options.interval).await;
                }
            })
            .await;
        match result {
            Err(crate::Error::Timeout(_)) => Err(crate::Error::Timeout(format!(
                "LayerZero message for {} not delivered (last status {}, last read error {})",
                lookup.source_txid,
                last_status.map_or_else(|| "not indexed".to_string(), |s| format!("{s:?}")),
                last_error.as_deref().unwrap_or("none")
            ))),
            other => other,
        }
    }
}

/// Exact names, as the daemon's `bridge_target_chain` spells them.
#[doc(alias = "getEid")]
pub fn chain_by_name(name: &str) -> Option<Usdt0Chain> {
    CHAINS.iter().find(|chain| chain.name == name).copied()
}
pub fn chain_by_eid(eid: u32) -> Option<Usdt0Chain> {
    CHAINS.iter().find(|chain| chain.eid == eid).copied()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usdt0Chain {
    pub name: &'static str,
    pub eid: u32,
    pub token: Address,
}

pub const CHAINS: &[Usdt0Chain] = &[
    Usdt0Chain {
        name: "Ethereum",
        eid: 30101,
        token: address!("0xdAC17F958D2ee523a2206206994597C13D831ec7"),
    },
    Usdt0Chain {
        name: "Polygon",
        eid: 30109,
        token: address!("0xc2132D05D31c914a87C6611C10748AEb04B58e8F"),
    },
    Usdt0Chain {
        name: "Arbitrum",
        eid: 30110,
        token: address!("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"),
    },
    Usdt0Chain {
        name: "Optimism",
        eid: 30111,
        token: address!("0x01bFF41798a0BcF287b996046Ca68b395DbC1071"),
    },
    Usdt0Chain {
        name: "Mantle",
        eid: 30181,
        token: address!("0x779Ded0c9e1022225f8E0630b35a9b54bE713736"),
    },
    Usdt0Chain {
        name: "Conflux eSpace",
        eid: 30212,
        token: address!("0xaf37E8B6C9ED7f6318979f56Fc287d76c30847ff"),
    },
    Usdt0Chain {
        name: "XLayer",
        eid: 30274,
        token: address!("0x779Ded0c9e1022225f8E0630b35a9b54bE713736"),
    },
    Usdt0Chain {
        name: "Sei",
        eid: 30280,
        token: address!("0x9151434b16b9763660705744891fA906F660EcC5"),
    },
    Usdt0Chain {
        name: "Flare",
        eid: 30295,
        token: address!("0xe7cd86e13AC4309349F30B3435a9d337750fC82D"),
    },
    Usdt0Chain {
        name: "Hedera",
        eid: 30316,
        token: address!("0x00000000000000000000000000000000009Ce723"),
    },
    Usdt0Chain {
        name: "Unichain",
        eid: 30320,
        token: address!("0x9151434b16b9763660705744891fA906F660EcC5"),
    },
    Usdt0Chain {
        name: "Morph",
        eid: 30322,
        token: address!("0xe7cd86e13AC4309349F30B3435a9d337750fC82D"),
    },
    Usdt0Chain {
        name: "Corn",
        eid: 30331,
        token: address!("0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb"),
    },
    Usdt0Chain {
        name: "Rootstock",
        eid: 30333,
        token: address!("0x779dED0C9e1022225F8e0630b35A9B54Be713736"),
    },
    Usdt0Chain {
        name: "Ink",
        eid: 30339,
        token: address!("0x0200C29006150606B650577BBE7B6248F58470c1"),
    },
    Usdt0Chain {
        name: "Berachain",
        eid: 30362,
        token: address!("0x779Ded0c9e1022225f8E0630b35a9b54bE713736"),
    },
    Usdt0Chain {
        name: "HyperEVM",
        eid: 30367,
        token: address!("0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb"),
    },
    Usdt0Chain {
        name: "Plasma",
        eid: 30383,
        token: address!("0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb"),
    },
    Usdt0Chain {
        name: "Monad",
        eid: 30390,
        token: address!("0xe7cd86e13AC4309349F30B3435a9d337750fC82D"),
    },
    Usdt0Chain {
        name: "Stable",
        eid: 30396,
        token: address!("0x779Ded0c9e1022225f8E0630b35a9b54bE713736"),
    },
    Usdt0Chain {
        name: "MegaETH",
        eid: 30398,
        token: address!("0xb8ce59fc3717ada4c02eadf9682a9e934f625ebb"),
    },
    Usdt0Chain {
        name: "Tempo",
        eid: 30410,
        token: address!("0x20C00000000000000000000014f22CA97301EB73"),
    },
];
