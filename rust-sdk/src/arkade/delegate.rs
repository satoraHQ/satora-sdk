//! Delegated batch settlement for spendable, expired and swept VHTLC outputs.
use super::execution::archive;
use super::execution::decode;
use super::execution::invalid;
use super::execution::merge_cosigned;
use super::execution::restore;
use super::execution::sign_psbt;
use super::execution::validate_info;
use super::execution::validate_vtxos;
use super::*;
use crate::SwapMetadata;
use ark_rs::core::batch::prepare_delegate_psbts;
use ark_rs::core::intent::Input;
use ark_rs::core::intent::Output;
use bitcoin::Sequence;
use bitcoin::TxOut;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArkadeDelegateState {
    pub operation: ArkadeSpend,
    pub destination: String,
    pub amount_sats: u64,
    pub intent_proof: String,
    pub intent_message: String,
    pub forfeit_psbts: Vec<String>,
    pub cosigner_pk: String,
    pub commitment_txid: Option<String>,
}
impl std::fmt::Debug for ArkadeDelegateState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArkadeDelegateState")
            .field("operation", &self.operation)
            .field("commitment_txid", &self.commitment_txid)
            .finish_non_exhaustive()
    }
}

impl Client {
    pub async fn delegate_cosigner_key(&self) -> Result<secp256k1::PublicKey> {
        #[derive(Deserialize)]
        struct Response {
            cosigner_pk: String,
        }
        let response = self
            .api_request(
                reqwest::Method::GET,
                self.base_url.join("/api/delegate/cosigner-pk")?,
            )
            .send()
            .await?;
        let response: Response = crate::client::check_status(response).await?.json().await?;
        response.cosigner_pk.parse().map_err(decode)
    }

    /// Settle a VHTLC through the server's Ark batch cosigner. The caller pins
    /// its public key (available from `delegate_cosigner_key`) before signing.
    /// Progress excludes preimages. Retry first reconciles spent input VTXOs.
    pub async fn delegate_arkade(
        &self,
        id: &str,
        backend: &dyn ArkadeBackend,
        chain: Option<&dyn crate::bitcoin::BitcoinBackend>,
        operation: ArkadeSpend,
        cosigner: secp256k1::PublicKey,
        options: ArkadeSpendOptions,
    ) -> Result<Txid> {
        tokio::time::timeout(options.timeout,async {
            let _execution=self.arkade_execution.lock().await;
            let stored=self.get_stored_swap(id)?.ok_or_else(||invalid("swap record missing"))?;
            if stored.metadata.arkade_pending.is_some() { return Err(invalid("an offchain Arkade spend is already persisted; resume it first")); }
            let htlc=self.arkade_htlc(id)?;let script=htlc.spend_script(operation)?;
            let info=backend.info().await?;validate_info(&htlc,&info)?;
            let destination=ArkAddress::decode(&options.destination).map_err(decode)?;
            if destination.encode()!=options.destination || destination.server()!=info.signer_pk.x_only_public_key().0 || options.destination.starts_with("ark1")!=(htlc.network==Network::Bitcoin) { return Err(invalid("Arkade destination network or server mismatch")); }
            let params=self.signer.as_ref().ok_or_else(||invalid("signer missing"))?.derive_swap_params(stored.key_index)?;
            let pair=Keypair::from_secret_key(&Secp256k1::new(),&secp256k1::SecretKey::from_slice(&params.secret).map_err(decode)?);
            let preimage=(operation==ArkadeSpend::Claim).then_some(params.preimage);
            let vtxos=backend.vtxos(htlc.address()).await?;validate_vtxos(&htlc,&vtxos)?;
            let mut state=if let Some(state)=stored.metadata.arkade_delegate {
                if state.operation!=operation || state.destination!=options.destination || state.cosigner_pk!=cosigner.to_string() { return Err(invalid("persisted Arkade delegation differs from this request")); }
                if let Some(hash)=&state.commitment_txid { return hash.parse().map_err(decode); }
                let proof=restore(&state.intent_proof,None)?;
                let inputs:Vec<_>=proof.unsigned_tx.input.iter().skip(1).map(|i|i.previous_output).collect();
                let spent:Vec<_>=inputs.iter().filter_map(|outpoint|vtxos.iter().find(|v|v.outpoint==*outpoint && v.is_spent).and_then(|v|v.settled_by)).collect();
                if !inputs.is_empty() && spent.len()==inputs.len() && spent.iter().all(|txid|*txid==spent[0]) {
                    let mut completed=state;
                    completed.commitment_txid=Some(spent[0].to_string());self.save_arkade_delegate(id,completed)?;
                    return Ok(spent[0]);
                }
                let message:serde_json::Value=serde_json::from_str(&state.intent_message).map_err(decode)?;
                let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(decode)?.as_secs();
                if message["expire_at"].as_u64().is_none_or(|expires|expires<=now) { return Err(invalid("delegated intent expired before its outcome was observed; reconcile its inputs before creating another intent")); }
                state
            } else {
                if operation==ArkadeSpend::Refund {
                    let chain=chain.ok_or_else(||invalid("Arkade refunds require a Bitcoin reader"))?;
                    if chain.network()!=htlc.network { return Err(invalid("Arkade refund network mismatch")); }
                    let clock=chain.observe(&Address::from_script(&htlc.script.script_pubkey(),htlc.network).map_err(decode)?).await?;
                    let lock=htlc.options.refund_locktime;
                    if !(if lock<500_000_000 {clock.height>=lock} else {clock.median_time_past>lock}) {return Err(invalid("Arkade refund timelock has not matured"));}
                }
                let list=VtxoList::new(info.dust,vtxos);
                let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(decode)?.as_secs();
                let available:Vec<_>=list.batch_settleable_at(&info,i64::try_from(now).map_err(decode)?,|_|Some(htlc.options.server)).collect();
                if available.is_empty() {return Err(invalid("no settleable Arkade VTXOs"));}
                let amount=available.iter().try_fold(0u64,|sum,v|sum.checked_add(v.amount.to_sat()).filter(|sum|*sum<=Amount::MAX_MONEY.to_sat()).ok_or_else(||invalid("Arkade amount overflow")))?;
                if operation==ArkadeSpend::Claim && amount<htlc.expected_amount.to_sat() {return Err(invalid("Arkade HTLC is underfunded"));}
                let control=htlc.script.taproot_spend_info().control_block(&(script.clone(),LeafVersion::TapScript)).ok_or_else(||invalid("Arkade control block missing"))?;
                let lock=(operation==ArkadeSpend::Refund).then(||bitcoin::absolute::LockTime::from_consensus(htlc.options.refund_locktime));
                let inputs=available.iter().map(|v|Input::new_with_extra_witness(v.outpoint,if lock.is_some() {Sequence::ENABLE_LOCKTIME_NO_RBF} else {Sequence::MAX},lock,TxOut {value:v.amount,script_pubkey:v.script.clone()},htlc.script.tapscripts(),(script.clone(),control.clone()),false,v.is_swept,vec![],preimage.map(|p|vec![p.to_vec()]).unwrap_or_default())).collect();
                let mut delegate=prepare_delegate_psbts(inputs,vec![Output::Offchain(TxOut {value:Amount::from_sat(amount),script_pubkey:destination.to_p2tr_script_pubkey()})],cosigner,&info.forfeit_address,info.dust).map_err(decode)?;
                // Match the TypeScript intent's explicit SIGHASH_ALL; forfeits
                // use ALL|ANYONECANPAY so the batch can append a connector input.
                for input in &mut delegate.intent.proof.inputs {input.sighash_type=Some(bitcoin::TapSighashType::All.into());}
                sign_psbt(&mut delegate.intent.proof,&pair,preimage)?;
                for forfeit in &mut delegate.forfeit_psbts {
                    if let Some(lock)=lock {forfeit.unsigned_tx.lock_time=lock;forfeit.unsigned_tx.input[0].sequence=Sequence::ENABLE_LOCKTIME_NO_RBF;}
                    sign_psbt(forfeit,&pair,preimage)?;
                }
                let mut state=ArkadeDelegateState {operation,destination:options.destination.clone(),amount_sats:amount,intent_proof:archive(&delegate.intent.proof),intent_message:delegate.intent.serialize_message().map_err(decode)?,forfeit_psbts:delegate.forfeit_psbts.iter().map(archive).collect(),cosigner_pk:cosigner.to_string(),commitment_txid:None};
                if operation==ArkadeSpend::CooperativeRefund {self.cosign_arkade_delegate(id,&mut state,&htlc).await?;}
                self.save_arkade_delegate(id,state.clone())?;
                state
            };
            let proof=restore(&state.intent_proof,preimage)?.to_string();
            let forfeits=state.forfeit_psbts.iter().map(|p|restore(p,preimage).map(|p|p.to_string())).collect::<Result<Vec<_>>>()?;
            let body=serde_json::json!({"intent_proof":proof,"intent_message":state.intent_message,"forfeit_psbts":forfeits,"cosigner_pk":state.cosigner_pk,"swap_id":preimage.map(|_|id),"preimage":preimage.map(hex::encode)});
            let response=self.api_request(reqwest::Method::POST,self.base_url.join("/api/delegate/settle")?).json(&body).send().await?;
            #[derive(Deserialize)]struct Response {commitment_txid:String}
            let response:Response=crate::client::check_status(response).await?.json().await?;
            let txid=response.commitment_txid.parse().map_err(decode)?;
            state.commitment_txid=Some(response.commitment_txid);self.save_arkade_delegate(id,state)?;
            Ok(txid)
        }).await.map_err(|_|Error::Timeout("Arkade delegated settlement timed out; reconcile the persisted intent".into()))?
    }
    fn save_arkade_delegate(&self, id: &str, state: ArkadeDelegateState) -> Result<()> {
        self.update_swap_metadata(
            id,
            SwapMetadata {
                arkade_delegate: Some(state),
                ..Default::default()
            },
        )?;
        Ok(())
    }
    async fn cosign_arkade_delegate(
        &self,
        id: &str,
        state: &mut ArkadeDelegateState,
        htlc: &ArkadeHtlc,
    ) -> Result<()> {
        if !matches!(
            self.get_stored_swap(id)?
                .ok_or_else(|| invalid("swap missing"))?
                .response,
            crate::types::GetSwapResponse::ArkadeToEvm(_)
        ) {
            return Err(invalid("cooperative delegation requires Arkade to EVM"));
        }
        let mut url = self.base_url.join("/api/swap/arkade-evm/")?;
        url.path_segments_mut()
            .map_err(|_| invalid("invalid URL"))?
            .pop_if_empty()
            .push(id)
            .push("collab-refund-delegate");
        let response=self.api_request(reqwest::Method::POST,url).json(&serde_json::json!({"intent_proof":state.intent_proof,"forfeit_psbts":state.forfeit_psbts})).send().await?;
        #[derive(Deserialize)]
        struct Response {
            intent_proof: String,
            forfeit_psbts: Vec<String>,
        }
        let response: Response = crate::client::check_status(response).await?.json().await?;
        let proof = merge_cosigned(
            &restore(&state.intent_proof, None)?,
            &restore(&response.intent_proof, None)?,
            htlc.options.receiver,
        )?;
        if state.forfeit_psbts.len() != response.forfeit_psbts.len() {
            return Err(invalid("delegate cosigner changed forfeit count"));
        }
        let forfeits = state
            .forfeit_psbts
            .iter()
            .zip(&response.forfeit_psbts)
            .map(|(a, b)| {
                merge_cosigned(
                    &restore(a, None)?,
                    &restore(b, None)?,
                    htlc.options.receiver,
                )
                .map(|p| archive(&p))
            })
            .collect::<Result<Vec<_>>>()?;
        state.intent_proof = archive(&proof);
        state.forfeit_psbts = forfeits;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum ArkadeSettlement {
    Offchain(ClaimReceipt),
    Delegated { commitment_txid: Txid },
}
impl Client {
    /// Select offchain execution for spendable VTXOs, delegated settlement for
    /// recoverable/mixed balances, and the persisted path when resuming.
    pub async fn settle_arkade(
        &self,
        id: &str,
        backend: &dyn ArkadeBackend,
        chain: Option<&dyn crate::bitcoin::BitcoinBackend>,
        operation: ArkadeSpend,
        cosigner: Option<secp256k1::PublicKey>,
        options: ArkadeSpendOptions,
    ) -> Result<ArkadeSettlement> {
        tokio::time::timeout(options.timeout, async {
            let stored = self
                .get_stored_swap(id)?
                .ok_or_else(|| invalid("swap record missing"))?;
            let delegated = if stored.metadata.arkade_delegate.is_some() {
                true
            } else if stored.metadata.arkade_pending.is_some() {
                false
            } else {
                self.vhtlc_balance(id, backend).await?.recoverable_sats > 0
            };
            if delegated {
                let cosigner = cosigner.ok_or_else(|| {
                    invalid("delegated settlement requires the expected cosigner public key")
                })?;
                let commitment_txid = self
                    .delegate_arkade(id, backend, chain, operation, cosigner, options)
                    .await?;
                Ok(ArkadeSettlement::Delegated { commitment_txid })
            } else {
                Ok(ArkadeSettlement::Offchain(
                    self.spend_arkade(id, backend, chain, operation, options)
                        .await?,
                ))
            }
        })
        .await
        .map_err(|_| Error::Timeout("Arkade settlement timed out".into()))?
    }
}
