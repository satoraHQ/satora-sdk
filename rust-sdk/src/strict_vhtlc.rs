use crate::Error;
use crate::Result;
use ark_rs::core::ArkAddress;
use ark_rs::core::UNSPENDABLE_KEY;
use bitcoin::Network;
use bitcoin::PublicKey;
use bitcoin::ScriptBuf;
use bitcoin::Sequence;
use bitcoin::XOnlyPublicKey;
use bitcoin::hashes::Hash;
use bitcoin::hashes::ripemd160;
use bitcoin::opcodes::all::*;
use bitcoin::taproot::TaprootBuilder;
use bitcoin::taproot::TaprootSpendInfo;
use std::str::FromStr;

pub const ARKADE_HTLC_SCRIPT_VERSION_STRICT: i64 = 1;

#[derive(Debug, Clone)]
struct TaprootScriptItem {
    script: ScriptBuf,
    weight: u32,
}

#[derive(Debug, Clone)]
enum TaprootTreeNode {
    Leaf {
        script: ScriptBuf,
        weight: u32,
    },
    Branch {
        left: Box<TaprootTreeNode>,
        right: Box<TaprootTreeNode>,
        weight: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StrictVhtlcOptions {
    pub sender: XOnlyPublicKey,
    pub receiver: XOnlyPublicKey,
    pub server: XOnlyPublicKey,
    pub preimage_hash: ripemd160::Hash,
    pub refund_locktime: u32,
    pub unilateral_claim_delay: Sequence,
    pub unilateral_refund_delay: Sequence,
    pub unilateral_refund_without_receiver_delay: Sequence,
}

impl StrictVhtlcOptions {
    fn validate(&self) -> Result<()> {
        if self.refund_locktime == 0 {
            return Err(Error::InvalidSwap(
                "refund locktime must be non-zero".into(),
            ));
        }
        if !self.unilateral_claim_delay.is_relative_lock_time()
            || self.unilateral_claim_delay.to_consensus_u32() == 0
        {
            return Err(Error::InvalidSwap(
                "unilateral claim delay must be a non-zero relative lock time".into(),
            ));
        }
        if !self.unilateral_refund_delay.is_relative_lock_time()
            || self.unilateral_refund_delay.to_consensus_u32() == 0
        {
            return Err(Error::InvalidSwap(
                "unilateral refund delay must be a non-zero relative lock time".into(),
            ));
        }
        if !self
            .unilateral_refund_without_receiver_delay
            .is_relative_lock_time()
            || self
                .unilateral_refund_without_receiver_delay
                .to_consensus_u32()
                == 0
        {
            return Err(Error::InvalidSwap(
                "unilateral refund without receiver delay must be a non-zero relative lock time"
                    .into(),
            ));
        }
        Ok(())
    }

    fn preimage_check_script(
        builder: bitcoin::script::Builder,
        preimage_hash: &ripemd160::Hash,
    ) -> bitcoin::script::Builder {
        builder
            .push_opcode(OP_SIZE)
            .push_int(32)
            .push_opcode(OP_EQUALVERIFY)
            .push_opcode(OP_HASH160)
            .push_slice(preimage_hash.as_byte_array())
            .push_opcode(OP_EQUAL)
            .push_opcode(OP_VERIFY)
    }

    pub fn claim_script(&self) -> ScriptBuf {
        Self::preimage_check_script(ScriptBuf::builder(), &self.preimage_hash)
            .push_x_only_key(&self.receiver)
            .push_opcode(OP_CHECKSIGVERIFY)
            .push_x_only_key(&self.server)
            .push_opcode(OP_CHECKSIG)
            .into_script()
    }

    pub fn refund_script(&self) -> ScriptBuf {
        ScriptBuf::builder()
            .push_x_only_key(&self.sender)
            .push_opcode(OP_CHECKSIGVERIFY)
            .push_x_only_key(&self.receiver)
            .push_opcode(OP_CHECKSIGVERIFY)
            .push_x_only_key(&self.server)
            .push_opcode(OP_CHECKSIG)
            .into_script()
    }

    pub fn refund_without_receiver_script(&self) -> ScriptBuf {
        ScriptBuf::builder()
            .push_int(self.refund_locktime as i64)
            .push_opcode(OP_CLTV)
            .push_opcode(OP_DROP)
            .push_x_only_key(&self.sender)
            .push_opcode(OP_CHECKSIGVERIFY)
            .push_x_only_key(&self.server)
            .push_opcode(OP_CHECKSIG)
            .into_script()
    }

    pub fn unilateral_claim_script(&self) -> ScriptBuf {
        Self::preimage_check_script(ScriptBuf::builder(), &self.preimage_hash)
            .push_int(self.unilateral_claim_delay.to_consensus_u32() as i64)
            .push_opcode(OP_CSV)
            .push_opcode(OP_DROP)
            .push_x_only_key(&self.receiver)
            .push_opcode(OP_CHECKSIG)
            .into_script()
    }

    pub fn unilateral_refund_script(&self) -> ScriptBuf {
        ScriptBuf::builder()
            .push_int(self.unilateral_refund_delay.to_consensus_u32() as i64)
            .push_opcode(OP_CSV)
            .push_opcode(OP_DROP)
            .push_x_only_key(&self.sender)
            .push_opcode(OP_CHECKSIGVERIFY)
            .push_x_only_key(&self.receiver)
            .push_opcode(OP_CHECKSIG)
            .into_script()
    }

    pub fn unilateral_refund_without_receiver_script(&self) -> ScriptBuf {
        ScriptBuf::builder()
            .push_int(
                self.unilateral_refund_without_receiver_delay
                    .to_consensus_u32() as i64,
            )
            .push_opcode(OP_CSV)
            .push_opcode(OP_DROP)
            .push_x_only_key(&self.sender)
            .push_opcode(OP_CHECKSIG)
            .into_script()
    }

    fn build_taproot(&self) -> Result<TaprootSpendInfo> {
        let internal_pubkey = PublicKey::from_str(UNSPENDABLE_KEY)
            .map_err(|e| Error::Decode(format!("invalid NUMS key: {e}")))?;
        let internal_key = XOnlyPublicKey::from(internal_pubkey);

        let scripts = vec![
            TaprootScriptItem {
                script: self.claim_script(),
                weight: 1,
            },
            TaprootScriptItem {
                script: self.refund_script(),
                weight: 1,
            },
            TaprootScriptItem {
                script: self.refund_without_receiver_script(),
                weight: 1,
            },
            TaprootScriptItem {
                script: self.unilateral_claim_script(),
                weight: 1,
            },
            TaprootScriptItem {
                script: self.unilateral_refund_script(),
                weight: 1,
            },
            TaprootScriptItem {
                script: self.unilateral_refund_without_receiver_script(),
                weight: 1,
            },
        ];

        let tree = Self::taproot_list_to_tree(scripts)?;
        let builder = Self::add_tree_to_builder(TaprootBuilder::new(), &tree, 0)?;
        let secp = bitcoin::secp256k1::Secp256k1::new();
        builder
            .finalize(&secp, internal_key)
            .map_err(|e| Error::Decode(format!("failed to finalize taproot: {e:?}")))
    }

    fn taproot_list_to_tree(scripts: Vec<TaprootScriptItem>) -> Result<TaprootTreeNode> {
        if scripts.is_empty() {
            return Err(Error::Decode("empty script list".into()));
        }
        let mut nodes: Vec<TaprootTreeNode> = scripts
            .into_iter()
            .map(|item| TaprootTreeNode::Leaf {
                script: item.script,
                weight: item.weight,
            })
            .collect();

        while nodes.len() >= 2 {
            nodes.sort_by_key(|b| std::cmp::Reverse(b.weight()));
            let b = nodes
                .pop()
                .ok_or_else(|| Error::Decode("missing node".into()))?;
            let a = nodes
                .pop()
                .ok_or_else(|| Error::Decode("missing node".into()))?;
            nodes.push(TaprootTreeNode::Branch {
                weight: a.weight() + b.weight(),
                left: Box::new(a),
                right: Box::new(b),
            });
        }

        nodes
            .into_iter()
            .next()
            .ok_or_else(|| Error::Decode("missing root".into()))
    }

    fn add_tree_to_builder(
        builder: TaprootBuilder,
        node: &TaprootTreeNode,
        depth: u8,
    ) -> Result<TaprootBuilder> {
        match node {
            TaprootTreeNode::Leaf { script, .. } => builder
                .add_leaf(depth, script.clone())
                .map_err(|e| Error::Decode(format!("failed to add leaf: {e}"))),
            TaprootTreeNode::Branch { left, right, .. } => {
                let builder = Self::add_tree_to_builder(builder, left, depth + 1)?;
                Self::add_tree_to_builder(builder, right, depth + 1)
            }
        }
    }
}

impl TaprootTreeNode {
    fn weight(&self) -> u32 {
        match self {
            Self::Leaf { weight, .. } | Self::Branch { weight, .. } => *weight,
        }
    }
}

pub struct StrictVhtlcScript {
    options: StrictVhtlcOptions,
    taproot_spend_info: TaprootSpendInfo,
    network: Network,
}

impl StrictVhtlcScript {
    pub fn new(options: StrictVhtlcOptions, network: Network) -> Result<Self> {
        options.validate()?;
        let taproot_spend_info = options.build_taproot()?;
        Ok(Self {
            options,
            taproot_spend_info,
            network,
        })
    }

    pub fn taproot_spend_info(&self) -> &TaprootSpendInfo {
        &self.taproot_spend_info
    }

    pub fn script_pubkey(&self) -> ScriptBuf {
        ScriptBuf::builder()
            .push_opcode(OP_PUSHNUM_1)
            .push_slice(self.taproot_spend_info.output_key().serialize())
            .into_script()
    }

    pub fn address(&self) -> ArkAddress {
        ArkAddress::new(
            self.network,
            self.options.server,
            self.taproot_spend_info().output_key(),
        )
    }

    pub fn claim_script(&self) -> ScriptBuf {
        self.options.claim_script()
    }

    pub fn tapscripts(&self) -> Vec<ScriptBuf> {
        vec![
            self.options.claim_script(),
            self.options.refund_script(),
            self.options.refund_without_receiver_script(),
            self.options.unilateral_claim_script(),
            self.options.unilateral_refund_script(),
            self.options.unilateral_refund_without_receiver_script(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;

    #[test]
    fn strict_vhtlc_matches_cross_language_vector() {
        let vector: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/strict-vhtlc-vectors.json"))
                .unwrap();
        let vector = &vector["strictArkadeVhtlcV1"];
        let scripts = &vector["scripts"];

        let sender = XOnlyPublicKey::from_str(vector["sender"].as_str().unwrap()).unwrap();
        let receiver = XOnlyPublicKey::from_str(vector["receiver"].as_str().unwrap()).unwrap();
        let server = XOnlyPublicKey::from_str(vector["server"].as_str().unwrap()).unwrap();
        let preimage_hash = ripemd160::Hash::from_slice(
            &hex::decode(vector["preimageHash"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        let vhtlc = StrictVhtlcScript::new(
            StrictVhtlcOptions {
                sender,
                receiver,
                server,
                preimage_hash,
                refund_locktime: vector["refundLocktime"].as_u64().unwrap() as u32,
                unilateral_claim_delay: Sequence::from_height(
                    vector["unilateralClaimDelay"].as_u64().unwrap() as u16,
                ),
                unilateral_refund_delay: Sequence::from_height(
                    vector["unilateralRefundDelay"].as_u64().unwrap() as u16,
                ),
                unilateral_refund_without_receiver_delay: Sequence::from_height(
                    vector["unilateralRefundWithoutReceiverDelay"]
                        .as_u64()
                        .unwrap() as u16,
                ),
            },
            Network::Regtest,
        )
        .unwrap();
        let tapscripts: Vec<String> = vhtlc
            .tapscripts()
            .iter()
            .map(|script| hex::encode(script.as_bytes()))
            .collect();

        assert_eq!(tapscripts[0], scripts["claim"].as_str().unwrap());
        assert_eq!(tapscripts[1], scripts["refund"].as_str().unwrap());
        assert_eq!(
            tapscripts[2],
            scripts["refundWithoutReceiver"].as_str().unwrap()
        );
        assert_eq!(tapscripts[3], scripts["unilateralClaim"].as_str().unwrap());
        assert_eq!(tapscripts[4], scripts["unilateralRefund"].as_str().unwrap());
        assert_eq!(
            tapscripts[5],
            scripts["unilateralRefundWithoutReceiver"].as_str().unwrap()
        );
        assert_eq!(
            hex::encode(vhtlc.script_pubkey().as_bytes()),
            vector["scriptPubKey"].as_str().unwrap()
        );
        assert_eq!(
            vhtlc.address().to_string(),
            vector["address"].as_str().unwrap()
        );
    }
}
