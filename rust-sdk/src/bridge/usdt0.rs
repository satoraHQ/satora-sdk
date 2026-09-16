//! USDT0 OFT encoding and LayerZero delivery tracking.
use super::BridgeApi;
use super::invalid;
use super::pending_json;
use crate::Result;
use crate::control::PollOptions;
use crate::evm::EvmWallet;
use crate::evm::funding::transaction;
use alloy::primitives::Address;
use alloy::primitives::B256;
use alloy::primitives::Bytes;
use alloy::primitives::U256;
use alloy::primitives::address;
use alloy::sol;
use alloy::sol_types::SolCall;
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeliveryStatus {
    Inflight,
    Confirming,
    Delivered,
    Failed,
    Blocked,
    PayloadStored,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub guid: B256,
    pub status: DeliveryStatus,
    pub source_txid: B256,
    pub destination_txid: Option<B256>,
    pub source_eid: u32,
    pub destination_eid: u32,
}
#[derive(Clone, Debug)]
pub struct MessageQuery {
    pub source_txid: B256,
    pub source_eid: u32,
    pub destination_eid: u32,
    /// Obtain from the source receipt's OFTSent event. Required to distinguish
    /// multiple transfers in a single transaction.
    pub guid: B256,
}
impl BridgeApi {
    pub async fn usdt0_status(&self, query: &MessageQuery) -> Result<Option<Delivery>> {
        #[derive(Deserialize)]
        struct Response {
            data: Vec<Message>,
        }
        #[derive(Deserialize)]
        struct Message {
            guid: B256,
            status: Status,
            pathway: Pathway,
            source: Side,
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
        }
        #[derive(Deserialize)]
        struct Side {
            tx: Option<Tx>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Tx {
            tx_hash: B256,
        }
        let url = self
            .layerzero
            .join(&format!("/v1/messages/tx/{}", query.source_txid))?;
        let Some(response) = pending_json::<Response>(self.http.get(url).send().await?).await?
        else {
            return Ok(None);
        };
        let mut matching = response.data.into_iter().filter(|m| m.guid == query.guid);
        let Some(message) = matching.next() else {
            return Ok(None);
        };
        if matching.next().is_some()
            || message.pathway.src_eid != query.source_eid
            || message.pathway.dst_eid != query.destination_eid
            || message.source.tx.as_ref().map(|tx| tx.tx_hash) != Some(query.source_txid)
        {
            return Err(invalid("LayerZero message identity mismatch"));
        }
        let destination_txid = message.destination.tx.map(|tx| tx.tx_hash);
        if message.status.name == DeliveryStatus::Delivered && destination_txid.is_none() {
            return Err(invalid(
                "delivered LayerZero message has no destination transaction",
            ));
        }
        Ok(Some(Delivery {
            guid: message.guid,
            status: message.status.name,
            source_txid: query.source_txid,
            destination_txid,
            source_eid: query.source_eid,
            destination_eid: query.destination_eid,
        }))
    }
    pub async fn wait_usdt0(
        &self,
        query: &MessageQuery,
        options: &PollOptions,
    ) -> Result<Delivery> {
        options
            .run(async {
                loop {
                    match self.usdt0_status(query).await {
                        Ok(Some(delivery)) => match delivery.status {
                            DeliveryStatus::Delivered => return Ok(delivery),
                            DeliveryStatus::Failed
                            | DeliveryStatus::Blocked
                            | DeliveryStatus::PayloadStored => {
                                return Err(invalid(format!(
                                    "LayerZero delivery requires recovery: {:?}",
                                    delivery.status
                                )));
                            }
                            _ => {}
                        },
                        Ok(None) | Err(crate::Error::Transport(_)) => {}
                        Err(error) => return Err(error),
                    }
                    tokio::time::sleep(options.interval).await;
                }
            })
            .await
    }
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
