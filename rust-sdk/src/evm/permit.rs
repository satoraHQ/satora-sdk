//! EIP-2612 helpers for relayed USDC approvals. The sponsored funding path
//! normally uses an approval inside its atomic batch instead.
use super::funding::transaction;
use super::funding::word;
use super::*;
use alloy::primitives::keccak256;
use alloy::sol;
use alloy::sol_types::Eip712Domain;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolStruct;
use serde_json::json;

sol! {
    struct Permit {address owner; address spender; uint256 value; uint256 nonce; uint256 deadline;}
    interface PermitToken {
        function DOMAIN_SEPARATOR() external view returns (bytes32);
        function nonces(address owner) external view returns (uint256);
        function permit(address owner, address spender, uint256 value, uint256 deadline, uint8 v, bytes32 r, bytes32 s) external;
    }
}
#[derive(Clone, Debug)]
pub struct PermitDomain {
    pub name: String,
    pub version: String,
    pub chain_id: u64,
    pub token: Address,
}
impl PermitDomain {
    pub fn separator(&self) -> B256 {
        Eip712Domain {
            name: Some(self.name.clone().into()),
            version: Some(self.version.clone().into()),
            chain_id: Some(U256::from(self.chain_id)),
            verifying_contract: Some(self.token),
            salt: None,
        }
        .hash_struct()
    }
    pub fn arbitrum_usdc() -> Self {
        Self {
            name: "USD Coin".into(),
            version: "2".into(),
            chain_id: 42161,
            token: crate::bridge::cctp::CctpChain::for_chain(42161)
                .expect("registered chain")
                .usdc,
        }
    }
}
#[derive(Clone, Debug)]
pub struct PermitApproval {
    pub domain: PermitDomain,
    pub spender: Address,
    pub amount: U256,
    pub deadline: u64,
}
/// A spending authorization: diagnostics deliberately omit its signature.
#[derive(Clone)]
pub struct SignedPermit {
    pub token: Address,
    pub owner: Address,
    pub spender: Address,
    pub amount: U256,
    pub nonce: U256,
    pub deadline: u64,
    pub signature: Signature,
}
impl std::fmt::Debug for SignedPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedPermit")
            .field("token", &self.token)
            .field("owner", &self.owner)
            .field("spender", &self.spender)
            .field("amount", &self.amount)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}
impl SignedPermit {
    pub fn calldata(&self) -> Bytes {
        PermitToken::permitCall {
            owner: self.owner,
            spender: self.spender,
            value: self.amount,
            deadline: U256::from(self.deadline),
            v: u8::from(self.signature.v()) + 27,
            r: self.signature.r().into(),
            s: self.signature.s().into(),
        }
        .abi_encode()
        .into()
    }
}
impl PermitApproval {
    pub fn typed_data(&self, owner: Address, nonce: U256) -> Result<EvmTypedData> {
        if self.domain.chain_id == 0
            || self.domain.token.is_zero()
            || owner.is_zero()
            || self.spender.is_zero()
            || self.amount.is_zero()
            || self.deadline == 0
        {
            return Err(invalid("invalid EIP-2612 approval"));
        }
        let permit = Permit {
            owner,
            spender: self.spender,
            value: self.amount,
            nonce,
            deadline: U256::from(self.deadline),
        };
        let digest = permit_digest(
            self.domain.separator(),
            owner,
            self.spender,
            self.amount,
            nonce,
            U256::from(self.deadline),
        );
        let payload = json!({"domain":{"name":self.domain.name,"version":self.domain.version,"chainId":self.domain.chain_id,"verifyingContract":self.domain.token},"primaryType":"Permit","types":{"EIP712Domain":[{"name":"name","type":"string"},{"name":"version","type":"string"},{"name":"chainId","type":"uint256"},{"name":"verifyingContract","type":"address"}],"Permit":[{"name":"owner","type":"address"},{"name":"spender","type":"address"},{"name":"value","type":"uint256"},{"name":"nonce","type":"uint256"},{"name":"deadline","type":"uint256"}]},"message":{"owner":permit.owner,"spender":permit.spender,"value":permit.value.to_string(),"nonce":permit.nonce.to_string(),"deadline":permit.deadline.to_string()}});
        Ok(EvmTypedData { payload, digest })
    }
    /// Fetch and verify the token domain and current nonce before prompting a
    /// wallet. Deadline is checked against chain time; the exact spender and
    /// bounded amount are signed, never an implicit unlimited approval.
    pub async fn sign(&self, wallet: &dyn EvmWallet) -> Result<SignedPermit> {
        if wallet.chain_id() != self.domain.chain_id {
            return Err(invalid("permit wallet chain mismatch"));
        }
        let separator = wallet
            .call(&transaction(
                wallet,
                self.domain.token,
                PermitToken::DOMAIN_SEPARATORCall {}.abi_encode(),
            ))
            .await?;
        if separator.len() != 32 || separator.as_ref() != self.domain.separator().as_slice() {
            return Err(invalid("permit token domain mismatch"));
        }
        let nonce = word(
            &wallet
                .call(&transaction(
                    wallet,
                    self.domain.token,
                    PermitToken::noncesCall {
                        owner: wallet.address(),
                    }
                    .abi_encode(),
                ))
                .await?,
        )?;
        if self.deadline <= wallet.block_timestamp().await? {
            return Err(invalid("permit deadline has expired"));
        }
        let typed = self.typed_data(wallet.address(), nonce)?;
        let signature = wallet.sign_typed_data(&typed).await?;
        verify_signature(&signature, typed.digest, wallet.address())?;
        Ok(SignedPermit {
            token: self.domain.token,
            owner: wallet.address(),
            spender: self.spender,
            amount: self.amount,
            nonce,
            deadline: self.deadline,
            signature,
        })
    }
}
/// Digest for integrations that already read DOMAIN_SEPARATOR and nonce.
pub fn permit_digest(
    domain_separator: B256,
    owner: Address,
    spender: Address,
    amount: U256,
    nonce: U256,
    deadline: U256,
) -> B256 {
    let permit = Permit {
        owner,
        spender,
        value: amount,
        nonce,
        deadline,
    };
    let mut encoded = Vec::with_capacity(66);
    encoded.extend([0x19, 0x01]);
    encoded.extend(domain_separator);
    encoded.extend(permit.eip712_hash_struct());
    keccak256(encoded)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permit_digest_matches_wallet_json_with_full_width_nonce() {
        let approval = PermitApproval {
            domain: PermitDomain::arbitrum_usdc(),
            spender: crate::aa::permit2::PERMIT2_ADDRESS,
            amount: U256::from(10_000_000),
            deadline: 2_000_000_000,
        };
        let owner = Address::repeat_byte(3);
        let data = approval.typed_data(owner, U256::MAX).unwrap();
        let dynamic: alloy_dyn_abi::TypedData = serde_json::from_value(data.payload).unwrap();
        assert_eq!(data.digest, dynamic.eip712_signing_hash().unwrap());
        assert_ne!(
            data.digest,
            approval.typed_data(owner, U256::ZERO).unwrap().digest
        );
        let mut changed = approval;
        changed.domain.chain_id = 1;
        assert_ne!(
            data.digest,
            changed.typed_data(owner, U256::MAX).unwrap().digest
        );
    }
}
