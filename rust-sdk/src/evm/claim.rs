use super::funding::transaction;
use super::htlc::EvmHtlc;
use super::htlc::typed_data;
use super::htlc::verify_version;
use super::*;
use crate::Client;
use crate::SwapMetadata;
use crate::types::EvmHtlcKind;
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
    interface NativeClaimCoordinator {
        function redeemAndExecute(bytes32 preimage, uint256 amount, address htlcSender, uint256 timelock, Call[] calls, address sweepToken, uint256 minAmountOut, address destination, uint8 v, bytes32 r, bytes32 s) external payable;
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
    kind: EvmHtlcKind,
}
impl ClaimMaterial {
    /// The native HTLC has no asset word: its Redeem struct omits `token` and its
    /// domain name differs, so a signature never verifies on the other family.
    fn typed_data(&self, options: &EvmClaimOptions, calls_hash: B256) -> Result<EvmTypedData> {
        let mut fields = vec![("preimage", "bytes32"), ("amount", "uint256")];
        let mut message = json!({"preimage":self.preimage,"amount":self.htlc.amount.to_string(),"sender":self.htlc.sender,"timelock":self.htlc.timelock.to_string(),"caller":options.coordinator,"destination":options.destination,"sweepToken":options.sweep_token,"minAmountOut":options.min_amount_out.to_string(),"callsHash":calls_hash});
        let name = match self.kind {
            EvmHtlcKind::Erc20 => {
                fields.push(("token", "address"));
                message["token"] = json!(self.htlc.token);
                "HTLCErc20"
            }
            EvmHtlcKind::Native => "HTLCNative",
        };
        fields.extend([
            ("sender", "address"),
            ("timelock", "uint256"),
            ("caller", "address"),
            ("destination", "address"),
            ("sweepToken", "address"),
            ("minAmountOut", "uint256"),
            ("callsHash", "bytes32"),
        ]);
        typed_data(
            name,
            self.version,
            self.htlc.chain_id,
            self.htlc.address,
            "Redeem",
            &fields,
            message,
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
        self.claim_material_checked(id, wallet, options, true).await
    }

    pub(crate) async fn verify_evm_claim_receipt(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: &EvmClaimOptions,
        receipt: &EvmReceipt,
    ) -> Result<()> {
        let material = self
            .claim_material_checked(id, wallet, options, false)
            .await?;
        material
            .htlc
            .verify_redeem_receipt(receipt, material.preimage)
    }

    async fn claim_material_checked(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: &EvmClaimOptions,
        verify_live: bool,
    ) -> Result<ClaimMaterial> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        if verify_live
            && (stored.metadata.evm_claim_started == Some(true)
                || stored.metadata.evm_claim_txid.is_some()
                || stored.metadata.evm_claim_userop_hash.is_some())
        {
            return Err(invalid(
                "claim already submitted; inspect the persisted transaction or operation",
            ));
        }
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
        let kind = context.kind;
        let htlc = EvmHtlc {
            chain_id: context.chain_id,
            address: context.htlc,
            hash_lock: context.hash,
            amount: decimal(field("evm_expected_sats")?)?,
            token: context.source_token,
            sender: context.server,
            claimer: context.owner,
            timelock: context.timelock,
        };
        if verify_live {
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
        }
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
            kind,
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
        self.claim_evm_with_native_fee(id, wallet, options, calls, U256::ZERO)
            .await
    }

    /// Claim with an explicit total native-value budget for reviewed bridge calls.
    /// The exact per-call values are included in the signed calls hash.
    pub async fn claim_evm_with_native_fee(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: EvmClaimOptions,
        calls: Vec<crate::types::DexCall>,
        max_native_fee: U256,
    ) -> Result<EvmReceipt> {
        tokio::time::timeout(options.timeout, async {
            let _execution = self.evm_execution.lock().await;
            let (material, tx) = self
                .prepare_evm_claim(id, wallet, &options, calls, max_native_fee)
                .await?;
            // The preimage is only sent to RPC after the funded identity and deadline checks.
            wallet.call(&tx).await?;
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    evm_claim_started: Some(true),
                    ..Default::default()
                },
            )?;
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

    async fn prepare_evm_claim(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: &EvmClaimOptions,
        calls: Vec<crate::types::DexCall>,
        max_native_fee: U256,
    ) -> Result<(ClaimMaterial, EvmTransaction)> {
        let material = self.claim_material(id, wallet, options).await?;
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
        let native_fee = calls.iter().try_fold(U256::ZERO, |total, call| {
            total
                .checked_add(call.value)
                .ok_or_else(|| invalid("native fee overflow"))
        })?;
        if native_fee > max_native_fee {
            return Err(invalid("native fees exceed the approved bridge budget"));
        }
        let typed = material.typed_data(options, keccak256(calls.abi_encode()))?;
        let signature = wallet.sign_typed_data(&typed).await?;
        verify_signature(&signature, typed.digest, wallet.address())?;
        let (v, r, s) = (
            u8::from(signature.v()) + 27,
            signature.r().into(),
            signature.s().into(),
        );
        let data = match material.kind {
            EvmHtlcKind::Erc20 => ClaimCoordinator::redeemAndExecuteCall {
                preimage: material.preimage,
                amount: material.htlc.amount,
                token: material.htlc.token,
                sender: material.htlc.sender,
                timelock: U256::from(material.htlc.timelock),
                calls,
                sweepToken: options.sweep_token,
                minAmountOut: options.min_amount_out,
                destination: options.destination,
                v,
                r,
                s,
            }
            .abi_encode(),
            EvmHtlcKind::Native => NativeClaimCoordinator::redeemAndExecuteCall {
                preimage: material.preimage,
                amount: material.htlc.amount,
                htlcSender: material.htlc.sender,
                timelock: U256::from(material.htlc.timelock),
                calls,
                sweepToken: options.sweep_token,
                minAmountOut: options.min_amount_out,
                destination: options.destination,
                v,
                r,
                s,
            }
            .abi_encode(),
        };
        let mut tx = transaction(wallet, options.coordinator, data);
        tx.value = native_fee;
        Ok((material, tx))
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
            // The daemon settles a native lock with a fixed shape: no calls, the coin
            // swept to the destination, at least the full amount.
            let (calls_hash, dex_calldata) = match material.kind {
                EvmHtlcKind::Native => {
                    if options.sweep_token != Address::ZERO || options.min_amount_out > material.htlc.amount {
                        return Err(invalid("native claim output does not match the locked coin/amount"));
                    }
                    options.min_amount_out = material.htlc.amount;
                    (keccak256(Vec::<Call>::new().abi_encode()), None)
                }
                EvmHtlcKind::Erc20 => {
                    let calldata = self.get_redeem_calldata(id,options.destination).await?;
                    let server_min = decimal(&calldata.min_amount_out)?;
                    if server_min < options.min_amount_out { return Err(invalid("server claim output is below the requested minimum")); }
                    options.min_amount_out = server_min;
                    (parse(&calldata.calls_hash)?, calldata.dex_calldata)
                }
            };
            let typed = material.typed_data(&options,calls_hash)?;
            let signature = wallet.sign_typed_data(&typed).await?;
            verify_signature(&signature,typed.digest,wallet.address())?;
            let stored = self.get_stored_swap(id)?.ok_or_else(|| invalid("swap record missing"))?;
            self.update_swap_metadata(id,SwapMetadata {evm_claim_started:Some(true),..Default::default()})?;
            let response = self.api_request(Method::POST,self.swap_action_url(id,"claim-gasless")?).json(&json!({
                "secret":material.preimage,"destination":options.destination,"v":u8::from(signature.v())+27,"r":format!("0x{:064x}",signature.r()),"s":format!("0x{:064x}",signature.s()),
                "dex_calldata":dex_calldata,"bridge_recipient":stored.metadata.bridge_recipient,"bridge_recipient_wallet":stored.metadata.bridge_recipient_wallet
            })).send().await?;
            let submitted: EvmSubmission = crate::client::check_status(response).await?.json().await?;
            if submitted.id != id { return Err(Error::Decode("claim response ID mismatch".into())); }
            let _: B256 = parse(&submitted.tx_hash)?;
            self.update_swap_metadata(id,SwapMetadata { evm_claim_txid:Some(submitted.tx_hash.clone()),target_address:Some(options.destination.to_string()),..Default::default() })?;
            Ok(submitted)
        }).await.map_err(|_| Error::Timeout("gasless EVM claim timed out".into()))?
    }
}

impl Client {
    /// Publish a locally authorized claim through EIP-7702 with optional gas
    /// sponsorship. The reader must expose the stored owner; its private key is
    /// recovered from the SDK seed, including imported TypeScript derivations.
    pub async fn claim_evm_userop(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: EvmClaimOptions,
        calls: Vec<crate::types::DexCall>,
        config: &crate::aa::UserOpConfig,
    ) -> Result<crate::aa::FundSwapReceipt> {
        self.claim_evm_userop_with_native_fee(id, wallet, options, calls, config, U256::ZERO)
            .await
    }

    /// Sponsored counterpart with a caller-approved native bridge fee cap.
    pub async fn claim_evm_userop_with_native_fee(
        &self,
        id: &str,
        wallet: &dyn EvmWallet,
        options: EvmClaimOptions,
        calls: Vec<crate::types::DexCall>,
        config: &crate::aa::UserOpConfig,
        max_native_fee: U256,
    ) -> Result<crate::aa::orchestrate::FundSwapReceipt> {
        tokio::time::timeout(options.timeout, async {
            let _execution = self.evm_execution.lock().await;
            let stored = self
                .get_stored_swap(id)?
                .ok_or_else(|| invalid("swap record missing"))?;
            if stored.metadata.evm_claim_txid.is_some()
                || stored.metadata.evm_claim_userop_hash.is_some()
            {
                return Err(invalid(
                    "claim already submitted; resume its persisted UserOperation or transaction",
                ));
            }
            let context = EvmContext::from_response(&stored.response)?;
            context.validate_wallet(wallet)?;
            let key = self
                .signer
                .as_ref()
                .ok_or_else(|| invalid("signer missing"))?
                .evm_key_for_address(stored.key_index, &context.owner.to_string())?;
            let (material, tx) = self
                .prepare_evm_claim(id, wallet, &options, calls, max_native_fee)
                .await?;
            wallet.call(&tx).await?;
            self.update_swap_metadata(
                id,
                SwapMetadata {
                    evm_claim_started: Some(true),
                    ..Default::default()
                },
            )?;
            let receipt = crate::aa::orchestrate::publish_with_config(
                key.secret_key,
                context.owner,
                context.chain_id,
                &[crate::aa::abi::Call {
                    target: tx.to,
                    value: tx.value,
                    callData: tx.data,
                }],
                config,
                &|hash| {
                    self.update_swap_metadata(
                        id,
                        SwapMetadata {
                            evm_claim_userop_hash: Some(hash.to_string()),
                            target_address: Some(options.destination.to_string()),
                            ..Default::default()
                        },
                    )?;
                    Ok(())
                },
            )
            .await?;
            if let Some(hash) = receipt.transaction_hash {
                self.update_swap_metadata(
                    id,
                    SwapMetadata {
                        evm_claim_txid: Some(hash.to_string()),
                        ..Default::default()
                    },
                )?;
                material.htlc.verify_redeem_receipt(
                    &wallet.wait_for_receipt(hash).await?,
                    material.preimage,
                )?;
            }
            Ok(receipt)
        })
        .await
        .map_err(|_| {
            Error::Timeout("sponsored EVM claim timed out; inspect the persisted operation".into())
        })?
    }

    /// Reconcile a lost claim submission response with its original transaction.
    /// This records identity only; verify its HTLC receipt before treating it as settled.
    pub async fn attach_evm_claim_transaction(&self, id: &str, hash: B256) -> Result<()> {
        let _guard = self.evm_execution.lock().await;
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        if hash.is_zero()
            || stored.metadata.evm_claim_started != Some(true)
            || stored.metadata.evm_claim_txid.is_some()
        {
            return Err(invalid("no unresolved claim submission to reconcile"));
        }
        self.update_swap_metadata(
            id,
            SwapMetadata {
                evm_claim_txid: Some(hash.to_string()),
                ..Default::default()
            },
        )
    }

    /// Resolve a submitted claim operation without releasing the preimage again.
    pub async fn resume_evm_claim_userop(
        &self,
        id: &str,
        bundler: &crate::aa::bundler::BundlerClient,
    ) -> Result<Option<B256>> {
        let _execution = self.evm_execution.lock().await;
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        if let Some(hash) = stored.metadata.evm_claim_txid {
            return Ok(Some(parse(&hash)?));
        }
        let hash = stored
            .metadata
            .evm_claim_userop_hash
            .ok_or_else(|| invalid("no persisted claim operation"))?;
        let Some(receipt) = bundler.get_user_operation_receipt(parse(&hash)?).await? else {
            return Ok(None);
        };
        if !receipt.success {
            return Err(invalid("claim UserOperation reverted"));
        }
        let hash = receipt.receipt.transaction_hash;
        self.update_swap_metadata(
            id,
            SwapMetadata {
                evm_claim_txid: Some(hash.to_string()),
                ..Default::default()
            },
        )?;
        Ok(Some(hash))
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
    // The struct name is part of the type hash, so the native shape keeps the
    // contract's `Redeem` name in its own namespace.
    mod native {
        alloy::sol! {
            struct Redeem { bytes32 preimage; uint256 amount; address sender; uint256 timelock; address caller; address destination; address sweepToken; uint256 minAmountOut; bytes32 callsHash; }
        }
    }
    fn fixture(version: u64, kind: EvmHtlcKind) -> (ClaimMaterial, EvmClaimOptions) {
        let htlc = EvmHtlc {
            chain_id: 137,
            address: Address::repeat_byte(1),
            hash_lock: B256::repeat_byte(2),
            amount: U256::from(100000),
            token: match kind {
                EvmHtlcKind::Erc20 => Address::repeat_byte(3),
                EvmHtlcKind::Native => Address::ZERO,
            },
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
        (
            ClaimMaterial {
                htlc,
                preimage: B256::repeat_byte(8),
                version,
                kind,
            },
            options,
        )
    }
    #[test]
    fn redeem_json_digest_matches_contract_encoding_for_each_domain_version() {
        for version in [4, 5, 6] {
            let (material, options) = fixture(version, EvmHtlcKind::Erc20);
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
    #[test]
    fn native_redeem_digest_omits_the_token_and_uses_the_native_domain() {
        let (material, options) = fixture(1, EvmHtlcKind::Native);
        let hash = B256::repeat_byte(9);
        let typed = material.typed_data(&options, hash).unwrap();
        let reference = native::Redeem {
            preimage: material.preimage,
            amount: material.htlc.amount,
            sender: material.htlc.sender,
            timelock: U256::from(material.htlc.timelock),
            caller: options.coordinator,
            destination: options.destination,
            sweepToken: options.sweep_token,
            minAmountOut: options.min_amount_out,
            callsHash: hash,
        };
        let domain = eip712_domain! {name:"HTLCNative",version:"1",chain_id:137,verifying_contract:material.htlc.address,};
        assert_eq!(typed.digest, reference.eip712_signing_hash(&domain));
        let erc20 = fixture(1, EvmHtlcKind::Erc20)
            .0
            .typed_data(&options, hash)
            .unwrap();
        assert_ne!(typed.digest, erc20.digest);
    }
}
