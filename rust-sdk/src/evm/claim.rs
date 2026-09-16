use super::funding::transaction;
use super::htlc::EvmHtlc;
use super::htlc::typed_data;
use super::htlc::verify_version;
use super::*;
use crate::Client;
use crate::SwapMetadata;
use alloy::primitives::keccak256;
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolValue;
use reqwest::Method;
use serde_json::json;
use std::time::Duration;

sol! {
    struct Call { address target; uint256 value; bytes callData; }
    interface ClaimCoordinator {
        function redeemAndExecute(bytes32 preimage, uint256 amount, address token, address sender, uint256 timelock, Call[] calls, address sweepToken, uint256 minAmountOut, address destination, uint8 v, bytes32 r, bytes32 s) external;
    }
}

#[derive(Clone, Debug)]
pub struct EvmClaimOptions {
    pub htlc: Address,
    pub coordinator: Address,
    pub destination: Address,
    pub sweep_token: Address,
    pub min_amount_out: U256,
    /// Refuse to reveal the preimage this close to the server's refund time.
    pub minimum_remaining_seconds: u64,
    pub timeout: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RedeemDexCall {
    pub to: String,
    pub data: String,
    pub value: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RedeemCalldata {
    pub calls_hash: String,
    pub min_amount_out: String,
    pub gasless_fee_sats: u64,
    pub dex_calldata: Option<RedeemDexCall>,
}

struct ClaimMaterial {
    htlc: EvmHtlc,
    preimage: B256,
    version: u64,
}
impl ClaimMaterial {
    fn typed_data(&self, options: &EvmClaimOptions, calls_hash: B256) -> Result<EvmTypedData> {
        typed_data(
            "HTLCErc20",
            self.version,
            self.htlc.chain_id,
            self.htlc.address,
            "Redeem",
            &[
                ("preimage", "bytes32"),
                ("amount", "uint256"),
                ("token", "address"),
                ("sender", "address"),
                ("timelock", "uint256"),
                ("caller", "address"),
                ("destination", "address"),
                ("sweepToken", "address"),
                ("minAmountOut", "uint256"),
                ("callsHash", "bytes32"),
            ],
            json!({"preimage":self.preimage,"amount":self.htlc.amount.to_string(),"token":self.htlc.token,"sender":self.htlc.sender,"timelock":self.htlc.timelock.to_string(),"caller":options.coordinator,"destination":options.destination,"sweepToken":options.sweep_token,"minAmountOut":options.min_amount_out.to_string(),"callsHash":calls_hash}),
        )
    }
}
impl Client {
    async fn claim_material(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: &EvmClaimOptions,
    ) -> Result<ClaimMaterial> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        let context = EvmContext::from_response(&stored.response)?;
        context.validate_wallet(wallet)?;
        if context.is_source || context.htlc != options.htlc {
            return Err(invalid("claim requires the configured EVM-target HTLC"));
        }
        let value =
            serde_json::to_value(&stored.response).map_err(|e| Error::Decode(e.to_string()))?;
        let field = |name: &str| {
            value[name]
                .as_str()
                .ok_or_else(|| invalid(format!("missing {name}")))
        };
        if parse::<Address>(field("evm_coordinator_address")?)? != options.coordinator {
            return Err(invalid("claim coordinator mismatch"));
        }
        let version = value["evm_htlc_version"]
            .as_u64()
            .ok_or_else(|| invalid("invalid HTLC version"))?;
        let htlc = EvmHtlc {
            chain_id: context.chain_id,
            address: context.htlc,
            hash_lock: context.hash,
            amount: decimal(field("evm_expected_sats")?)?,
            token: parse(field("wbtc_address")?)?,
            sender: context.server,
            claimer: context.owner,
            timelock: context.timelock,
        };
        if !htlc.is_active(wallet).await? {
            return Err(invalid("expected HTLC is not funded and active on chain"));
        }
        let remaining = htlc
            .timelock
            .saturating_sub(wallet.block_timestamp().await?);
        if remaining == 0 || remaining < options.minimum_remaining_seconds {
            return Err(invalid("insufficient time to claim before server refund"));
        }
        verify_version(wallet, htlc.address, version).await?;
        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| Error::InvalidSigner("claim requires the swap seed".into()))?;
        let params = signer.derive_swap_params(stored.key_index)?;
        if B256::from(params.hash_lock) != htlc.hash_lock {
            return Err(invalid("derived preimage does not match the on-chain HTLC"));
        }
        Ok(ClaimMaterial {
            htlc,
            preimage: B256::from(params.preimage),
            version,
        })
    }

    /// Claim through a wallet-submitted coordinator transaction. DEX calls must be
    /// freshly fetched for this coordinator and reviewed against the recipient and
    /// minimum output in `options`; their exact hash is bound into the signature.
    /// An empty list claims the locked token directly.
    pub async fn claim_evm_with_wallet(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: EvmClaimOptions,
        calls: Vec<crate::types::DexCall>,
    ) -> Result<EvmReceipt> {
        tokio::time::timeout(options.timeout, async {
            let _execution = self.evm_execution.lock().await;
            let material = self.claim_material(id, wallet, &options).await?;
            if calls.is_empty()
                && (options.sweep_token != material.htlc.token
                    || options.min_amount_out > material.htlc.amount)
            {
                return Err(invalid(
                    "direct claim output does not match locked token/amount",
                ));
            }
            let calls: Vec<Call> = calls
                .iter()
                .map(|c| {
                    Ok(Call {
                        target: parse(&c.target)?,
                        value: decimal(&c.value)?,
                        callData: parse(&c.data)?,
                    })
                })
                .collect::<Result<_>>()?;
            if calls.iter().any(|c| !c.value.is_zero()) {
                return Err(invalid("native-fee calls require bridge execution"));
            }
            let typed = material.typed_data(&options, keccak256(calls.abi_encode()))?;
            let signature = wallet.sign_typed_data(&typed).await?;
            verify_signature(&signature, typed.digest, wallet.address())?;
            let call = ClaimCoordinator::redeemAndExecuteCall {
                preimage: material.preimage,
                amount: material.htlc.amount,
                token: material.htlc.token,
                sender: material.htlc.sender,
                timelock: U256::from(material.htlc.timelock),
                calls,
                sweepToken: options.sweep_token,
                minAmountOut: options.min_amount_out,
                destination: options.destination,
                v: u8::from(signature.v()) + 27,
                r: signature.r().into(),
                s: signature.s().into(),
            };
            let tx = transaction(wallet, options.coordinator, call.abi_encode());
            // The preimage is only sent to RPC after the funded identity and deadline checks.
            wallet.call(&tx).await?;
            let hash = wallet.send_transaction(&tx).await?;
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    evm_claim_txid: Some(hash.to_string()),
                    target_address: Some(options.destination.to_string()),
                    ..Default::default()
                },
            )?;
            let receipt = wallet.wait_for_receipt(hash).await?;
            material
                .htlc
                .verify_redeem_receipt(&receipt, material.preimage)?;
            Ok(receipt)
        })
        .await
        .map_err(|_| Error::Timeout("EVM claim timed out; inspect its stored transaction".into()))?
    }

    pub async fn get_redeem_calldata(
        &self,
        id: &str,
        destination: Address,
    ) -> Result<RedeemCalldata> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        let mut query = vec![("destination", destination.to_string())];
        if let Some(recipient) = stored.metadata.bridge_recipient {
            query.push(("bridge_recipient", recipient));
        }
        if let Some(wallet) = stored.metadata.bridge_recipient_wallet {
            query.push(("bridge_recipient_wallet", wallet));
        }
        let response = self
            .api_request(
                Method::GET,
                self.swap_action_url(id, "redeem-and-swap-calldata")?,
            )
            .query(&query)
            .send()
            .await?;
        Ok(crate::client::check_status(response).await?.json().await?)
    }

    /// Authorize a server-submitted claim after verifying funding and the signing
    /// domain on chain. The destination, sweep token, minimum output, and exact
    /// server call hash are bound into the EIP-712 signature.
    pub async fn claim_evm_gasless(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        mut options: EvmClaimOptions,
    ) -> Result<EvmSubmission> {
        tokio::time::timeout(options.timeout,async {
            let _execution = self.evm_execution.lock().await;
            let material = self.claim_material(id,wallet,&options).await?;
            let calldata = self.get_redeem_calldata(id,options.destination).await?;
            let server_min = decimal(&calldata.min_amount_out)?;
            if server_min < options.min_amount_out { return Err(invalid("server claim output is below the requested minimum")); }
            options.min_amount_out = server_min;
            let typed = material.typed_data(&options,parse(&calldata.calls_hash)?)?;
            let signature = wallet.sign_typed_data(&typed).await?;
            verify_signature(&signature,typed.digest,wallet.address())?;
            let stored = self.get_stored_swap(id)?.ok_or_else(|| invalid("swap record missing"))?;
            let response = self.api_request(Method::POST,self.swap_action_url(id,"claim-gasless")?).json(&json!({
                "secret":material.preimage,"destination":options.destination,"v":u8::from(signature.v())+27,"r":format!("0x{:064x}",signature.r()),"s":format!("0x{:064x}",signature.s()),
                "dex_calldata":calldata.dex_calldata,"bridge_recipient":stored.metadata.bridge_recipient,"bridge_recipient_wallet":stored.metadata.bridge_recipient_wallet
            })).send().await?;
            let submitted: EvmSubmission = crate::client::check_status(response).await?.json().await?;
            if submitted.id != id { return Err(Error::Decode("claim response ID mismatch".into())); }
            let _: B256 = parse(&submitted.tx_hash)?;
            self.update_swap_metadata(id,SwapMetadata { evm_claim_txid:Some(submitted.tx_hash.clone()),target_address:Some(options.destination.to_string()),..Default::default() })?;
            Ok(submitted)
        }).await.map_err(|_| Error::Timeout("gasless EVM claim timed out".into()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolStruct;
    use alloy::sol_types::eip712_domain;
    sol! {
        struct Redeem { bytes32 preimage; uint256 amount; address token; address sender; uint256 timelock; address caller; address destination; address sweepToken; uint256 minAmountOut; bytes32 callsHash; }
    }
    #[test]
    fn redeem_json_digest_matches_contract_encoding_for_each_domain_version() {
        for version in [4, 5, 6] {
            let htlc = EvmHtlc {
                chain_id: 137,
                address: Address::repeat_byte(1),
                hash_lock: B256::repeat_byte(2),
                amount: U256::from(100000),
                token: Address::repeat_byte(3),
                sender: Address::repeat_byte(4),
                claimer: Address::repeat_byte(5),
                timelock: 1900000000,
            };
            let options = EvmClaimOptions {
                htlc: htlc.address,
                coordinator: Address::repeat_byte(6),
                destination: Address::repeat_byte(7),
                sweep_token: htlc.token,
                min_amount_out: htlc.amount,
                minimum_remaining_seconds: 300,
                timeout: Duration::from_secs(10),
            };
            let material = ClaimMaterial {
                htlc,
                preimage: B256::repeat_byte(8),
                version,
            };
            let hash = B256::repeat_byte(9);
            let typed = material.typed_data(&options, hash).unwrap();
            let reference = Redeem {
                preimage: material.preimage,
                amount: material.htlc.amount,
                token: material.htlc.token,
                sender: material.htlc.sender,
                timelock: U256::from(material.htlc.timelock),
                caller: options.coordinator,
                destination: options.destination,
                sweepToken: options.sweep_token,
                minAmountOut: options.min_amount_out,
                callsHash: hash,
            };
            let domain = eip712_domain! {name:"HTLCErc20",version:version.to_string(),chain_id:137,verifying_contract:material.htlc.address,};
            assert_eq!(typed.digest, reference.eip712_signing_hash(&domain));
            assert!(!format!("{typed:?}").contains(&material.preimage.to_string()));
        }
    }
}
