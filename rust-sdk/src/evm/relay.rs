//! Server-relayed Permit2 funding (`POST /swap/{id}/fund-gasless`): the depositor
//! only signs, and the server submits the funding and pays its gas.
use super::funding::FundingCall;
use super::funding::FundingOptions;
use super::funding::Permit2FundingData;
use super::funding::TokenAllowance;
use super::funding::UNRESOLVED_FUNDING;
use super::funding::now;
use super::funding::transaction;
use super::funding::word;
use super::htlc::EvmHtlc;
use super::permit::PermitToken;
use super::permit::permit_digest;
use super::recovery::RecoveryToken;
use super::refund::EvmSubmission;
use super::refund::first_verified;
use super::refund::verify_funding_receipt;
use super::*;
use crate::Client;
use crate::StoredSwap;
use crate::SwapMetadata;
use crate::aa::abi::Call;
use crate::aa::abi::IERC20;
use crate::aa::kernel::KERNEL_DELEGATION_TARGET;
use crate::aa::kernel::erc1271_wrapped_digest;
use crate::aa::kernel::wrap_erc1271_signature;
use crate::aa::permit2::PERMIT2_ADDRESS;
use crate::aa::signing::sign_hash;
use crate::logging::sdk_log;
use crate::signer::EvmKey;
use crate::storage::RelayFundingJournal;
use crate::storage::RelayFundingOutcome;
use crate::types::GetSwapResponse;
use alloy::primitives::keccak256;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolValue;
use rand::RngCore;
use reqwest::Method;
use serde_json::json;
use std::time::Duration;
use tokio::time::Instant;
use url::Url;

alloy::sol! {
    interface Permit2Nonces {
        function nonceBitmap(address owner, uint256 wordPosition) external view returns (uint256);
    }
}

/// Chain time past a signature's deadline before its unspent nonce counts as dead,
/// so a lagging node cannot report a nonce as dead while a block could still spend it.
const EXPIRY_MARGIN: u64 = 120;

/// What `tokio::time::timeout` waits for a duration too large to add to an instant.
const FAR_FUTURE: Duration = Duration::from_secs(86_400 * 365 * 30);

#[derive(Clone, Debug)]
pub struct RelayFundingOptions {
    /// Contracts, call targets and input limit to require on top of the stored swap,
    /// never instead of it. Either way the server's current copy of the swap has to
    /// match the stored one, and the payload has to name the stored coordinator,
    /// which must also be the one persisted for the swap. For EVM to Bitcoin and EVM
    /// to Lightning the payload has to lock the stored `wbtc_address`; EVM to Arkade
    /// responses name no lock token, so with `None` the payload's is signed. Pinned
    /// contracts that differ from the stored swap are refused. Calls on the source
    /// token are decoded instead of listed as targets.
    pub funding: Option<FundingOptions>,
    /// Largest relay fee the server may take from the input. Only Ethereum charges one.
    pub max_relay_fee: U256,
    /// Node for the balance, allowance, permit and receipt reads. `None` uses the
    /// chain's default.
    pub node_rpc_url: Option<Url>,
    /// How long the signatures stay valid.
    pub permit_deadline: Duration,
    /// Bounds the whole call, including the relay's wait for its receipt. A pinned
    /// `FundingOptions::timeout` applies when it is shorter.
    pub timeout: Duration,
}

impl RelayFundingOptions {
    /// Trust the stored swap, sign for 30 minutes as the TypeScript client does, and
    /// allow three minutes for the call.
    pub fn new(max_relay_fee: U256) -> Self {
        Self {
            funding: None,
            max_relay_fee,
            node_rpc_url: None,
            permit_deadline: Duration::from_secs(30 * 60),
            timeout: Duration::from_secs(180),
        }
    }
    fn budget(&self) -> Duration {
        self.funding
            .as_ref()
            .map_or(self.timeout, |pinned| pinned.timeout.min(self.timeout))
    }
}

#[derive(Clone, Debug)]
pub struct RelayFunding {
    pub tx_hash: B256,
    pub htlc: EvmHtlc,
    /// Whether the relay was also sent an EIP-2612 approval of Permit2.
    pub eip2612_permit: bool,
}

#[derive(Clone, Debug)]
pub enum RelayFundingState {
    Funded(RelayFunding),
    /// Not proven yet. With `nonce_consumed` the swap was funded under a hash this
    /// client does not know; it must never be funded again.
    Pending {
        nonce_consumed: bool,
        deadline: u64,
    },
    /// Past the deadline with the nonce unspent. The funding reservation is released.
    Expired,
}

#[derive(Deserialize)]
struct Eip2612Data {
    supported: bool,
    nonce: u64,
    domain_separator: String,
}

#[derive(Deserialize)]
struct RelayCalldata {
    #[serde(flatten)]
    funding: Permit2FundingData,
    #[serde(default)]
    eip2612: Option<Eip2612Data>,
    #[serde(default)]
    relay_fee: Option<String>,
    /// Absent when the server's code lookup failed; null for a plain EOA.
    #[serde(default, deserialize_with = "present")]
    depositor_delegation: Option<Option<Address>>,
}

fn present<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Option<Address>>, D::Error> {
    Option::<Address>::deserialize(deserializer).map(Some)
}

#[derive(Serialize)]
struct RelayRequest<'a> {
    permit2_nonce: String,
    permit2_deadline: u64,
    permit2_signature: Bytes,
    calls: &'a [FundingCall],
    #[serde(skip_serializing_if = "Option::is_none")]
    eip2612_permit: Option<Value>,
}

struct RelaySwap {
    context: EvmContext,
    coordinator: Address,
    /// EVM to Arkade responses omit it.
    lock_token: Option<Address>,
    source_amount: U256,
    gasless: bool,
}

impl RelaySwap {
    fn from_response(response: &GetSwapResponse) -> Result<Self> {
        let (coordinator, lock_token, amount, gasless) = match response {
            GetSwapResponse::EvmToArkade(r) => (
                &r.evm_coordinator_address,
                None,
                &r.source_amount,
                r.gasless,
            ),
            GetSwapResponse::EvmToBitcoin(r) => (
                &r.evm_coordinator_address,
                Some(&r.wbtc_address),
                &r.source_amount,
                r.gasless,
            ),
            GetSwapResponse::EvmToLightning(r) => (
                &r.evm_coordinator_address,
                Some(&r.wbtc_address),
                &r.source_amount,
                r.gasless,
            ),
            _ => return Err(invalid("relay funding requires an EVM-sourced swap")),
        };
        Ok(Self {
            context: EvmContext::from_response(response)?,
            coordinator: parse(coordinator)?,
            lock_token: lock_token.map(|token| parse(token)).transpose()?,
            source_amount: decimal(amount)?,
            gasless,
        })
    }
}

/// The coordinator runs these calls holding the pulled input, so calls on the source
/// token are decoded: only the fee transfer to the server and approvals bounded by
/// the input may touch it.
fn check_relay_calls(
    calls: &[Call],
    context: &EvmContext,
    amount: U256,
    relay_fee: Option<U256>,
    max_relay_fee: U256,
    allowed: Option<&[Address]>,
) -> Result<()> {
    let listed = |target: &Address| allowed.is_none_or(|allowed| allowed.contains(target));
    for (index, call) in calls.iter().enumerate() {
        if !call.value.is_zero() {
            return Err(invalid("funding contains a native-token transfer"));
        }
        if call.target != context.source_token {
            if !listed(&call.target) {
                return Err(invalid("funding contains an unapproved call target"));
            }
            continue;
        }
        if index == 0
            && let Ok(fee) = RecoveryToken::transferCall::abi_decode(&call.callData)
        {
            if fee.to != context.server
                || fee.amount > max_relay_fee
                || relay_fee.is_some_and(|quoted| quoted != fee.amount)
            {
                return Err(invalid(
                    "relay fee exceeds the limit or is not paid to the server",
                ));
            }
            continue;
        }
        let approve = IERC20::approveCall::abi_decode(&call.callData).map_err(|_| {
            invalid("funding calls the source token for more than the fee and an approval")
        })?;
        if approve.amount > amount || !listed(&approve.spender) {
            return Err(invalid(
                "funding approves an unapproved spender or more than the input",
            ));
        }
    }
    Ok(())
}

/// Outcomes after which the relay may have the request, so another attempt has to
/// wait for reconciliation.
fn may_be_submitted(outcome: RelayFundingOutcome) -> bool {
    matches!(
        outcome,
        RelayFundingOutcome::Sending
            | RelayFundingOutcome::Accepted
            | RelayFundingOutcome::Uncertain
            | RelayFundingOutcome::Funded
    )
}

fn kernel_delegation(target: Address) -> Result<bool> {
    if target != KERNEL_DELEGATION_TARGET {
        return Err(invalid(
            "depositor is EIP-7702-delegated to code this SDK cannot sign for",
        ));
    }
    Ok(true)
}

async fn nonce_spent(reader: &dyn EvmWallet, owner: Address, nonce: U256) -> Result<bool> {
    let bitmap = reader
        .call(&transaction(
            reader,
            PERMIT2_ADDRESS,
            Permit2Nonces::nonceBitmapCall {
                owner,
                wordPosition: nonce >> 8_usize,
            }
            .abi_encode(),
        ))
        .await?;
    Ok(word(&bitmap)?.bit(usize::from(nonce.byte(0))))
}

/// Proves the receipt funded this swap through the journal's coordinator and spent
/// its Permit2 nonce.
async fn prove_relay(
    reader: &dyn EvmWallet,
    context: &EvmContext,
    journal: &RelayFundingJournal,
    receipt: &EvmReceipt,
) -> Result<EvmHtlc> {
    let coordinator = parse(&journal.coordinator)?;
    let htlc = verify_funding_receipt(
        reader,
        context,
        receipt,
        coordinator,
        parse(&journal.lock_token)?,
        Some(coordinator),
    )
    .await
    .map_err(|(_, error)| error)?;
    if !nonce_spent(reader, context.owner, decimal(&journal.permit2_nonce)?).await? {
        return Err(invalid(
            "relay funding did not spend this swap's Permit2 nonce",
        ));
    }
    Ok(htlc)
}

async fn connect_reader(
    key: &EvmKey,
    context: &EvmContext,
    options: &RelayFundingOptions,
) -> Result<RpcWallet> {
    let url = match &options.node_rpc_url {
        Some(url) => url.clone(),
        None => {
            let default = crate::types::KnownChain::from_wire_str(&context.chain_id.to_string())
                .and_then(|chain| chain.default_node_rpc_url())
                .ok_or_else(|| invalid("swap chain has no default node RPC; set node_rpc_url"))?;
            Url::parse(default).map_err(|e| invalid(e.to_string()))?
        }
    };
    RpcWallet::connect(key, context.chain_id, url).await
}

impl Client {
    /// Fund an EVM-sourced gasless swap through the server relay, like the
    /// TypeScript `fundSwapGasless`: the swap's EVM key signs a Permit2 transfer and,
    /// without a Permit2 allowance, an unlimited EIP-2612 approval of Permit2.
    ///
    /// Before signing, the payload must match the stored swap and the persisted
    /// coordinator, and a pinned `funding` on top of that (see
    /// [`RelayFundingOptions::funding`]). Its source-token calls are decoded, the
    /// relay fee is capped, and the permit domain and nonce are read from chain.
    ///
    /// The attempt is persisted in `SwapMetadata::evm_fund_relay` before the request
    /// leaves, and every attempt signs the swap's one Permit2 nonce, so at most one
    /// can fund it. After an error the journal says what to do. With no journal, or
    /// one at `Rejected`, `Reverted` or `Expired`, the call can be repeated. At
    /// `Sending`, `Accepted` or `Uncertain` the relay may have the request: resolve
    /// it with [`Self::reconcile_gasless_relay_funding`], never by sending again.
    /// The funding is recorded once its receipt shows this swap's HTLC creation.
    pub async fn fund_swap_gasless_relay(
        &self,
        id: &str,
        options: RelayFundingOptions,
    ) -> Result<RelayFunding> {
        self.fund_relay_within(id, None, &options).await
    }

    /// Resolve a relay funding whose outcome is unknown, without sending anything.
    /// `Expired` releases the reservation so that any funding path may run again.
    pub async fn reconcile_gasless_relay_funding(
        &self,
        id: &str,
        options: RelayFundingOptions,
    ) -> Result<RelayFundingState> {
        tokio::time::timeout(options.budget(), async {
            let _execution = self.evm_execution.lock().await;
            self.reconcile_relay(id, None, &options).await
        })
        .await
        .map_err(|_| Error::Timeout("relay funding reconciliation timed out".into()))?
    }

    pub(super) async fn fund_relay_within(
        &self,
        id: &str,
        reader: Option<&dyn EvmWallet>,
        options: &RelayFundingOptions,
    ) -> Result<RelayFunding> {
        let budget = options.budget();
        let deadline = Instant::now()
            .checked_add(budget)
            .unwrap_or_else(|| Instant::now() + FAR_FUTURE);
        let funded = tokio::time::timeout(budget, async {
            let _execution = self.evm_execution.lock().await;
            self.fund_relay(id, reader, options, deadline).await
        })
        .await;
        funded.unwrap_or_else(|_| {
            // An unreadable record may hide a request that left.
            let may_be_sent = self.get_stored_swap(id).map_or(true, |stored| {
                stored
                    .and_then(|stored| stored.metadata.evm_fund_relay)
                    .is_some_and(|journal| may_be_submitted(journal.outcome))
            });
            Err(Error::Timeout(if may_be_sent {
                "relay funding timed out and its request may have been sent; reconcile it \
                 before another attempt"
                    .into()
            } else {
                "relay funding timed out before its request was sent; it can be repeated".into()
            }))
        })
    }

    fn relay_key(&self, stored: &StoredSwap, context: &EvmContext) -> Result<EvmKey> {
        self.signer
            .as_ref()
            .ok_or_else(|| Error::InvalidSigner("relay funding requires the swap seed".into()))?
            .evm_key_for_address(stored.key_index, &context.owner.to_string())
    }

    fn save_relay(&self, id: &str, journal: &RelayFundingJournal) -> Result<()> {
        self.update_swap_metadata(
            id,
            SwapMetadata {
                evm_fund_relay: Some(journal.clone()),
                ..Default::default()
            },
        )
    }

    fn relay_failed(
        &self,
        id: &str,
        mut journal: RelayFundingJournal,
        refused: bool,
        error: Error,
    ) -> Result<RelayFunding> {
        journal.outcome = if refused {
            RelayFundingOutcome::Rejected
        } else {
            RelayFundingOutcome::Uncertain
        };
        self.save_relay(id, &journal)?;
        Err(error)
    }

    fn relay_funded(
        &self,
        id: &str,
        mut journal: RelayFundingJournal,
        hash: B256,
        htlc: EvmHtlc,
    ) -> Result<RelayFunding> {
        journal.tx_hash = Some(hash.to_string());
        journal.outcome = RelayFundingOutcome::Funded;
        self.update_swap_metadata(
            id,
            SwapMetadata {
                evm_fund_txid: Some(hash.to_string()),
                evm_fund_relay: Some(journal.clone()),
                ..Default::default()
            },
        )?;
        Ok(RelayFunding {
            tx_hash: hash,
            htlc,
            eip2612_permit: journal.eip2612_permit,
        })
    }

    pub(super) async fn fund_relay(
        &self,
        id: &str,
        reader: Option<&dyn EvmWallet>,
        options: &RelayFundingOptions,
        deadline: Instant,
    ) -> Result<RelayFunding> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("recover or create the swap before funding"))?;
        let metadata = &stored.metadata;
        if metadata.evm_fund_txid.is_some()
            || metadata.evm_fund_userop_hash.is_some()
            || metadata
                .bridge_operations
                .as_ref()
                .is_some_and(|operations| operations.contains_key("cctp_fund"))
        {
            return Err(invalid(
                "funding already submitted; inspect its receipt before another attempt",
            ));
        }
        let previous = metadata.evm_fund_relay.clone();
        let reserved = metadata.evm_fund_started == Some(true);
        match previous.as_ref().map(|journal| journal.outcome) {
            None | Some(RelayFundingOutcome::Expired) if reserved => {
                return Err(invalid(UNRESOLVED_FUNDING));
            }
            Some(outcome) if may_be_submitted(outcome) => {
                return Err(invalid(
                    "relay funding may already be submitted; reconcile it before another attempt",
                ));
            }
            _ => {}
        }
        let swap = RelaySwap::from_response(&stored.response)?;
        let current = RelaySwap::from_response(&self.get_swap_details(id).await?)?;
        if current.context != swap.context
            || current.coordinator != swap.coordinator
            || current.lock_token != swap.lock_token
        {
            return Err(invalid("server swap details differ from the stored swap"));
        }
        if !current.gasless {
            return Err(invalid("relay funding requires a swap created as gasless"));
        }
        let context = &swap.context;
        if context.kind == crate::types::EvmHtlcKind::Native {
            return Err(invalid("the relay cannot fund a native lock"));
        }
        if metadata
            .evm_coordinator_address
            .as_ref()
            .is_some_and(|pinned| !pinned.eq_ignore_ascii_case(&swap.coordinator.to_string()))
        {
            return Err(invalid(
                "coordinator differs from the one persisted for this swap",
            ));
        }
        if let Some(pinned) = &options.funding
            && (pinned.coordinator != swap.coordinator
                || pinned.htlc != context.htlc
                || swap
                    .lock_token
                    .is_some_and(|stored| stored != pinned.lock_token))
        {
            return Err(invalid(
                "funding payload does not match the swap and configured contracts",
            ));
        }
        super::verify_derived_hash_lock(self, &stored, context)?;
        let key = self.relay_key(&stored, context)?;
        let connected;
        let reader: &dyn EvmWallet = match reader {
            Some(reader) => reader,
            None => {
                connected = connect_reader(&key, context, options).await?;
                &connected
            }
        };
        context.validate_wallet(reader)?;
        let owner = context.owner;

        let response = self
            .api_request(
                Method::GET,
                self.swap_action_url(id, "swap-and-lock-calldata-permit2")?,
            )
            .send()
            .await?;
        let calldata: RelayCalldata = crate::client::check_status(response).await?.json().await?;
        let payload = &calldata.funding;
        let lock_token = match (&options.funding, swap.lock_token) {
            (Some(pinned), _) => pinned.lock_token,
            (None, Some(stored)) => stored,
            // Without a stored or pinned token the signed one is used: the coordinator
            // can only lock the token in the witness, and nothing else is at risk.
            (None, None) => parse(&payload.lock_token_address)?,
        };
        payload.validate_identity(context, swap.coordinator, lock_token)?;
        let amount = decimal(&payload.source_amount)?;
        // A bridged deposit arrives net of the bridge fee, hence a bound, not equality.
        let limit = options
            .funding
            .as_ref()
            .map_or(swap.source_amount, |pinned| {
                pinned.max_source_amount.min(swap.source_amount)
            });
        if amount.is_zero() || amount > limit {
            return Err(invalid("funding amount exceeds the approved limit"));
        }
        let calls = payload.decoded_calls()?;
        if keccak256(calls.abi_encode()) != parse::<B256>(&payload.calls_hash)? {
            return Err(invalid("funding calls hash mismatch"));
        }
        check_relay_calls(
            &calls,
            context,
            amount,
            calldata.relay_fee.as_deref().map(decimal).transpose()?,
            options.max_relay_fee,
            options
                .funding
                .as_ref()
                .map(|pinned| pinned.allowed_call_targets.as_slice()),
        )?;
        let permit_deadline = now()?
            .checked_add(options.permit_deadline.as_secs())
            .ok_or_else(|| invalid("deadline overflow"))?;
        if permit_deadline >= context.timelock {
            return Err(invalid(
                "the signatures would stay valid past the refund timelock",
            ));
        }

        let read = |to: Address, data: Vec<u8>| async move {
            word(&reader.call(&transaction(reader, to, data)).await?)
        };
        let token = context.source_token;
        if read(token, IERC20::balanceOfCall { account: owner }.abi_encode()).await? < amount {
            return Err(invalid("insufficient token balance"));
        }
        let allowance = read(
            token,
            TokenAllowance::allowanceCall {
                owner,
                spender: PERMIT2_ADDRESS,
            }
            .abi_encode(),
        )
        .await?;
        let permit = if allowance < amount {
            let server = calldata
                .eip2612
                .as_ref()
                .filter(|data| data.supported)
                .ok_or_else(|| {
                    invalid("the source token has no Permit2 allowance and no EIP-2612 permit")
                })?;
            let separator = B256::from(
                read(token, PermitToken::DOMAIN_SEPARATORCall {}.abi_encode())
                    .await?
                    .to_be_bytes::<32>(),
            );
            let nonce = read(token, PermitToken::noncesCall { owner }.abi_encode()).await?;
            if parse::<B256>(&server.domain_separator)? != separator
                || U256::from(server.nonce) != nonce
            {
                return Err(invalid(
                    "server permit domain or nonce differs from the chain",
                ));
            }
            Some(permit_digest(
                separator,
                owner,
                PERMIT2_ADDRESS,
                U256::MAX,
                nonce,
                U256::from(permit_deadline),
            ))
        } else {
            None
        };

        let delegated = match calldata.depositor_delegation {
            Some(None) => false,
            Some(Some(target)) => kernel_delegation(target)?,
            None => match reader.code(owner).await {
                Ok(code) => match code.as_ref() {
                    [] => false,
                    [0xef, 0x01, 0x00, target @ ..] if target.len() == 20 => {
                        kernel_delegation(Address::from_slice(target))?
                    }
                    _ => return Err(invalid("depositor has code that is not a delegation")),
                },
                // As in the TypeScript client: a plain signature from a delegated
                // depositor fails the relay's dry-run before anything is broadcast.
                Err(error) => {
                    sdk_log!(
                        self.log.clone().swap_id(id),
                        Warn,
                        "client.isKernelDelegated.codeLookupFailed",
                        "Depositor code lookup failed; signing without the Kernel envelope",
                        "address" => owner.to_string(),
                        "error" => error.to_string()
                    );
                    false
                }
            },
        };
        let sign = |digest: B256| -> Result<Signature> {
            let signed = if delegated {
                erc1271_wrapped_digest(digest, owner, context.chain_id)
            } else {
                digest
            };
            verify_signature(sign_hash(&key.secret_key, signed)?, signed, owner)
        };
        let envelope = |signature: Signature| -> Bytes {
            if delegated {
                wrap_erc1271_signature(&signature.as_bytes())
            } else {
                Bytes::from(signature.as_bytes())
            }
        };
        let nonce = match &previous {
            Some(journal) => decimal(&journal.permit2_nonce)?,
            None => {
                let mut nonce = [0u8; 32];
                rand::rngs::OsRng.fill_bytes(&mut nonce);
                U256::from_be_bytes(nonce)
            }
        };
        let digest = payload
            .typed_data(context.chain_id, nonce, U256::from(permit_deadline))?
            .digest;
        let permit2_signature = envelope(sign(digest)?);
        let eip2612_permit = match permit {
            None => None,
            Some(digest) => {
                let signature = sign(digest)?;
                let value = U256::MAX.to_string();
                Some(if delegated {
                    json!({"signature": envelope(signature), "value": value, "deadline": permit_deadline})
                } else {
                    json!({
                        "v": u8::from(signature.v()) + 27,
                        "r": format!("0x{:064x}", signature.r()),
                        "s": format!("0x{:064x}", signature.s()),
                        "value": value,
                        "deadline": permit_deadline,
                    })
                })
            }
        };

        let mut journal = RelayFundingJournal {
            permit2_nonce: nonce.to_string(),
            // Every earlier signature with this nonce stays spendable until its own
            // deadline, so expiry waits for the latest one.
            permit2_deadline: previous.as_ref().map_or(permit_deadline, |journal| {
                journal.permit2_deadline.max(permit_deadline)
            }),
            depositor: owner.to_string(),
            coordinator: swap.coordinator.to_string(),
            lock_token: lock_token.to_string(),
            eip2612_permit: eip2612_permit.is_some(),
            tx_hash: None,
            outcome: RelayFundingOutcome::Sending,
        };
        self.update_swap_metadata(
            id,
            SwapMetadata {
                evm_fund_relay: Some(journal.clone()),
                evm_fund_started: Some(true),
                evm_coordinator_address: Some(swap.coordinator.to_string()),
                ..Default::default()
            },
        )?;
        let url = self.swap_action_url(id, "fund-gasless")?;
        let request = RelayRequest {
            permit2_nonce: nonce.to_string(),
            permit2_deadline: permit_deadline,
            permit2_signature,
            calls: &payload.calls,
            eip2612_permit,
        };
        // The relay answers after its funding receipt, so the operation deadline
        // bounds this request instead of the client-wide timeout.
        let sent = self
            .api_request(Method::POST, url.clone())
            .timeout(deadline.saturating_duration_since(Instant::now()))
            .json(&request)
            .send()
            .await;
        let submitted = match sent {
            Err(error) => {
                // A connect error after a redirect comes from a later hop, after the
                // first one read the body.
                let refused = (error.is_connect() || error.is_builder())
                    && error.url().is_none_or(|sent_to| *sent_to == url);
                return self.relay_failed(id, journal, refused, error.into());
            }
            Ok(response) => {
                let status = response.status();
                match crate::client::check_status(response).await {
                    // The relay answers 4xx only before it broadcasts the funding.
                    Err(error) => {
                        return self.relay_failed(id, journal, status.is_client_error(), error);
                    }
                    Ok(response) => response.json::<EvmSubmission>().await,
                }
            }
        };
        let hash = submitted.map_err(Error::from).and_then(|submitted| {
            if submitted.id != id {
                return Err(Error::Decode("relay response ID mismatch".into()));
            }
            parse::<B256>(&submitted.tx_hash)
        });
        let hash = match hash {
            Ok(hash) => hash,
            Err(error) => return self.relay_failed(id, journal, false, error),
        };
        journal.tx_hash = Some(hash.to_string());
        journal.outcome = RelayFundingOutcome::Accepted;
        self.save_relay(id, &journal)?;

        let receipt = reader.wait_for_receipt(hash).await?;
        if receipt.transaction_hash != hash {
            return Err(invalid("funding receipt transaction hash mismatch"));
        }
        if !receipt.success {
            journal.outcome = RelayFundingOutcome::Reverted;
            self.save_relay(id, &journal)?;
            return Err(invalid(format!(
                "relay funding transaction {hash} reverted"
            )));
        }
        let htlc = prove_relay(reader, context, &journal, &receipt).await?;
        self.relay_funded(id, journal, hash, htlc)
    }

    pub(super) async fn reconcile_relay(
        &self,
        id: &str,
        reader: Option<&dyn EvmWallet>,
        options: &RelayFundingOptions,
    ) -> Result<RelayFundingState> {
        let stored = self
            .get_stored_swap(id)?
            .ok_or_else(|| invalid("swap record missing"))?;
        let mut journal = stored
            .metadata
            .evm_fund_relay
            .clone()
            .ok_or_else(|| invalid("no relay funding to reconcile"))?;
        if journal.outcome == RelayFundingOutcome::Expired {
            return Ok(RelayFundingState::Expired);
        }
        let context = EvmContext::from_response(&stored.response)?;
        let key = self.relay_key(&stored, &context)?;
        let connected;
        let reader: &dyn EvmWallet = match reader {
            Some(reader) => reader,
            None => {
                connected = connect_reader(&key, &context, options).await?;
                &connected
            }
        };
        context.validate_wallet(reader)?;
        let server = self
            .server_funding_hash(id, "client.reconcileGaslessRelay.serverLookupFailed")
            .await;
        let proven = {
            let (context, journal) = (&context, &journal);
            let verify = |hash: String| async move {
                let unavailable = |error| (true, error);
                let expected = parse::<B256>(&hash).map_err(unavailable)?;
                let receipt = reader
                    .wait_for_receipt(expected)
                    .await
                    .map_err(unavailable)?;
                if receipt.transaction_hash != expected {
                    return Err(unavailable(invalid(
                        "funding receipt transaction hash mismatch",
                    )));
                }
                let htlc = prove_relay(reader, context, journal, &receipt)
                    .await
                    .map_err(unavailable)?;
                Ok((expected, htlc))
            };
            match (journal.tx_hash.clone(), server) {
                (Some(recorded), Some(server)) if !recorded.eq_ignore_ascii_case(&server) => {
                    first_verified(verify(recorded), verify(server)).await.ok()
                }
                (Some(hash), _) | (None, Some(hash)) => verify(hash).await.ok(),
                (None, None) => None,
            }
        };
        if let Some((hash, htlc)) = proven {
            return self
                .relay_funded(id, journal, hash, htlc)
                .map(RelayFundingState::Funded);
        }
        let nonce = decimal(&journal.permit2_nonce)?;
        let pending = |nonce_consumed| RelayFundingState::Pending {
            nonce_consumed,
            deadline: journal.permit2_deadline,
        };
        if nonce_spent(reader, context.owner, nonce).await? {
            return Ok(pending(true));
        }
        // Chain time first, then the nonce again: a block at or after that time can
        // no longer accept any of the signatures.
        let chain_time = reader.block_timestamp().await?;
        if chain_time <= journal.permit2_deadline.saturating_add(EXPIRY_MARGIN) {
            return Ok(pending(false));
        }
        if nonce_spent(reader, context.owner, nonce).await? {
            return Ok(pending(true));
        }
        journal.outcome = RelayFundingOutcome::Expired;
        self.update_swap_metadata(
            id,
            SwapMetadata {
                evm_fund_relay: Some(journal),
                evm_fund_started: Some(false),
                ..Default::default()
            },
        )?;
        Ok(RelayFundingState::Expired)
    }
}
