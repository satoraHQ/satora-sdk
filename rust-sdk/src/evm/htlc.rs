use super::funding::transaction;
use super::funding::word;
use super::*;
use alloy::primitives::keccak256;
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolValue;
use serde_json::json;

sol! {
    event SwapRedeemed(bytes32 indexed preimageHash, bytes32 indexed key, bytes32 preimage);
    event SwapRefunded(bytes32 indexed preimageHash, bytes32 indexed key);
    interface Htlc {
        function VERSION() external view returns (uint8);
        function isActive(bytes32 preimageHash, uint256 amount, address token, address sender, address claimAddress, uint256 timelock) external view returns (bool);
        function refund(bytes32 preimageHash, uint256 amount, address token, address claimAddress, uint256 timelock) external;
        function swapState(bytes32 key) external view returns (uint8 state, bytes32 preimage);
    }
    interface Coordinator {
        function VERSION() external view returns (uint8);
        function deposits(bytes32 key) external view returns (address);
        function refundTo(bytes32 preimageHash, uint256 amount, address token, address claimAddress, uint256 timelock) external;
    }
}

/// Fully specified on-chain HTLC identity, independent of API status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvmHtlc {
    pub chain_id: u64,
    pub address: Address,
    pub hash_lock: B256,
    pub amount: U256,
    pub token: Address,
    pub sender: Address,
    pub claimer: Address,
    pub timelock: u64,
}
impl EvmHtlc {
    pub fn key(&self) -> B256 {
        keccak256(
            (
                self.hash_lock,
                self.amount,
                self.token,
                self.sender,
                self.claimer,
                U256::from(self.timelock),
            )
                .abi_encode(),
        )
    }
    pub fn verify_redeem_receipt(&self, receipt: &EvmReceipt, preimage: B256) -> Result<()> {
        use alloy::sol_types::SolEvent;
        let found = receipt
            .logs
            .iter()
            .filter(|log| log.address == self.address)
            .any(|log| {
                SwapRedeemed::decode_raw_log_validate(log.topics.iter().copied(), &log.data)
                    .is_ok_and(|event| {
                        event.preimageHash == self.hash_lock
                            && event.key == self.key()
                            && event.preimage == preimage
                    })
            });
        if !receipt.success || !found {
            return Err(invalid(
                "receipt has no matching successful HTLC redemption",
            ));
        }
        Ok(())
    }

    pub fn verify_refund_receipt(&self, receipt: &EvmReceipt) -> Result<()> {
        use alloy::sol_types::SolEvent;
        let found = receipt
            .logs
            .iter()
            .filter(|log| log.address == self.address)
            .any(|log| {
                SwapRefunded::decode_raw_log_validate(log.topics.iter().copied(), &log.data)
                    .is_ok_and(|event| {
                        event.preimageHash == self.hash_lock && event.key == self.key()
                    })
            });
        if !receipt.success || !found {
            return Err(invalid("receipt has no matching successful HTLC refund"));
        }
        Ok(())
    }

    pub async fn is_active(&self, wallet: &dyn EvmWallet) -> Result<bool> {
        if wallet.chain_id() != self.chain_id {
            return Err(invalid("HTLC and reader chain mismatch"));
        }
        let call = Htlc::isActiveCall {
            preimageHash: self.hash_lock,
            amount: self.amount,
            token: self.token,
            sender: self.sender,
            claimAddress: self.claimer,
            timelock: U256::from(self.timelock),
        };
        let value = word(
            &wallet
                .call(&transaction(wallet, self.address, call.abi_encode()))
                .await?,
        )?;
        match value {
            U256::ZERO => Ok(false),
            U256::ONE => Ok(true),
            _ => Err(Error::Decode("invalid ABI boolean".into())),
        }
    }
}

pub(crate) async fn verify_version(
    wallet: &dyn EvmWallet,
    contract: Address,
    expected: u64,
) -> Result<()> {
    if expected == 0 {
        return Err(invalid("contract version must be positive"));
    }
    let version = wallet
        .call(&transaction(
            wallet,
            contract,
            Htlc::VERSIONCall {}.abi_encode(),
        ))
        .await?;
    if word(&version)? != U256::from(expected) {
        return Err(invalid(
            "on-chain contract version differs from signing domain",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn typed_data(
    name: &str,
    version: u64,
    chain_id: u64,
    contract: Address,
    primary: &str,
    fields: &[(&str, &str)],
    message: Value,
) -> Result<EvmTypedData> {
    let fields: Vec<_> = fields
        .iter()
        .map(|(name, kind)| json!({"name":name,"type":kind}))
        .collect();
    let payload = json!({
        "domain":{"name":name,"version":version.to_string(),"chainId":chain_id,"verifyingContract":contract},
        "primaryType":primary,"types":{
            "EIP712Domain":[{"name":"name","type":"string"},{"name":"version","type":"string"},{"name":"chainId","type":"uint256"},{"name":"verifyingContract","type":"address"}],
            (primary):fields
        },"message":message
    });
    let data: alloy_dyn_abi::TypedData =
        serde_json::from_value(payload.clone()).map_err(|e| Error::Decode(e.to_string()))?;
    let digest = data
        .eip712_signing_hash()
        .map_err(|e| Error::Decode(e.to_string()))?;
    Ok(EvmTypedData { payload, digest })
}
