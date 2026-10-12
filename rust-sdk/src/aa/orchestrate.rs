//! End-to-end gasless-funding orchestration: takes a swap's backend
//! calldata + the depositor's per-swap key, drives the full ERC-4337 +
//! EIP-7702 flow against a bundler + paymaster + node RPC, and returns
//! the userOpHash plus (when available) the on-chain transaction hash.
//!
//! Step ordering:
//!
//! 1. Build the Permit2 witness signature (Kernel-V3.3-wrapped under 7702), so the signature can be
//!    embedded in the coordinator call.
//! 2. Encode the outer batch (`approve(Permit2, 0)`, `approve(Permit2, source_amount)`,
//!    `HTLCCoordinator.executeAndCreateWithPermit2(..., permit2_sig)`) and wrap it in Kernel's
//!    `execute(BATCH_MODE, …)`. That's `callData`.
//! 3. Look up the userOp `nonce` via `EntryPoint.getNonce(sender, 0)` and the EOA's `tx_nonce` via
//!    `eth_getTransactionCount(sender)`.
//! 4. Sign the EIP-7702 authorization tuple (chainId, delegation target, tx_nonce) — attached to
//!    the userOp on submission.
//! 5. Build a skeleton `PackedUserOperation` (sender, nonce, callData, gas/paymaster placeholders,
//!    gas-estimation stub signature).
//! 6. `pm_getPaymasterStubData` → fills paymaster fields. **Skipped** when no paymaster is
//!    configured (the userOp's `paymaster*` fields stay empty and the sender's prefund covers gas).
//! 7. Read gas price (`eth_gasPrice` for max-fee; static floor for the priority component —
//!    bundler-specific tuning is out of scope here).
//! 8. `eth_estimateUserOperationGas` (with auth + stub paymaster) → fills the gas-limit fields.
//! 9. `pm_getPaymasterData` (unless stub came back `isFinal: true`, or there's no paymaster) →
//!    fills the real paymaster fields.
//! 10. Compute `userOpHash` and sign it with EIP-191 (Kernel's 7702 validator does
//!     `ECDSA.recover(toEthSignedMessageHash(…), sig)`).
//! 11. `eth_sendUserOperation` (with auth). The returned userOpHash must equal the one signed in
//!     step 10.
//! 12. Poll `eth_getUserOperationReceipt` until mined (bounded), then confirm on the node that the
//!     reported transaction emitted the operation's `UserOperationEvent`.
//!
//! All RPC calls inherit the [`crate::aa::bundler::BundlerCasing`] chosen on the clients —
//! the orchestration is bundler-agnostic.

use crate::aa::abi::Call;
use crate::aa::abi::IERC20;
use crate::aa::abi::IEntryPoint;
use crate::aa::abi::IHTLCCoordinator;
use crate::aa::abi::PermitTransferFrom;
use crate::aa::abi::TokenPermissions;
use crate::aa::bundler::BundlerClient;
use crate::aa::bundler::SignedEip7702Authorization;
use crate::aa::client_ext::GasOverrides;
use crate::aa::kernel;
use crate::aa::paymaster::PaymasterClient;
use crate::aa::permit2::PERMIT2_ADDRESS;
use crate::aa::permit2::PermitWitnessParams;
use crate::aa::permit2::permit2_digest;
use crate::aa::signing;
use crate::aa::userop::user_op_hash;
use crate::error::Error;
use crate::error::Result;
use crate::evm::EvmReceipt;
use alloy::eips::eip7702::Authorization;
use alloy::primitives::Address;
use alloy::primitives::B256;
use alloy::primitives::Bytes;
use alloy::primitives::U256;
use alloy::primitives::Uint;
use alloy::providers::Provider;
use alloy::providers::RootProvider;
use alloy::rpc::types::TransactionRequest;
use alloy::rpc::types::erc4337::PackedUserOperation;
use alloy::sol_types::SolCall;
use alloy::sol_types::SolEvent;
use alloy::sol_types::SolValue;
use rand::RngCore;
use rand::rngs::OsRng;
use serde_json::Value;
use std::time::Duration;
use tracing::info;

/// Stub `signature` placeholder for gas-estimation passes — matches
/// ZeroDev's `DUMMY_ECDSA_SIG` exactly (see `@zerodev/sdk/constants.ts`).
///
/// The layout matters: `s` must start with `0x7a` (< `0x7f`) so it
/// sits below `secp256k1n/2` ("low-s"). A high-s stub makes Solady's
/// `ECDSA.recover` revert with `InvalidSignature()` (`0x8baa579f`)
/// during validateUserOp simulation, manifesting as `AA23 reverted`
/// from the bundler. Low-s lets recover return *some* (wrong) address,
/// the validator returns `SIG_VALIDATION_FAILED`, and the bundler
/// proceeds with gas estimation as expected.
const STUB_USEROP_SIGNATURE: [u8; 65] = [
    // r (32 bytes): 15 × 0xff + 0xf0 + 16 × 0x00
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xf0,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    // s (32 bytes): 0x7a + 31 × 0xaa  — low-s, see comment above
    0x7a, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa,
    0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa,
    // v = 0x1c (28)
    0x1c,
];

/// Minimum priority fee floor. Bundlers reject `0` and the spread
/// between base-fee and effective-fee on Arbitrum is small enough that
/// 0.001 gwei works as a sensible default. Override by post-hoc gas-
/// price overrides if you need to tune.
const MIN_PRIORITY_FEE_WEI: u128 = 1_000_000;

/// Permit2 deadline horizon — the depositor's signature is only valid
/// for this long after construction. 30 minutes matches the TS SDK.
const PERMIT2_DEADLINE_HORIZON: Duration = Duration::from_secs(30 * 60);

/// Polling interval + cap for `eth_getUserOperationReceipt`. Tuned for
/// real-chain block times (Arbitrum ~250ms, Polygon ~2s, Anvil 1s) —
/// 1s × 30 = 30s total is more than enough for the userOp to land on
/// any chain we target. Bundler/network issues that take longer than
/// this would have surfaced as RPC errors during gas estimation or
/// submission first.
const RECEIPT_POLL_INTERVAL: Duration = Duration::from_secs(1);
const RECEIPT_POLL_ATTEMPTS: u32 = 30;

/// Everything `fund_swap` needs from the backend's
/// `/swap/{id}/swap-and-lock-calldata-userop` response, plus the
/// depositor key. Decoupled from the wire response struct so a future
/// SDK endpoint type can map onto it without changing this layer.
#[derive(Clone)]
pub struct FundSwapInputs {
    // From the backend response
    pub coordinator_address: Address,
    pub source_token_address: Address,
    pub source_amount: U256,
    pub lock_token_address: Address,
    pub preimage_hash: B256,
    pub claim_address: Address,
    pub timelock: u64,
    pub calls: Vec<Call>,
    pub calls_hash: B256,
    pub entry_point: Address,
    pub delegation_target: Address,

    // From the SDK signer + builder
    /// The depositor's per-swap secp256k1 secret. The matching EOA is
    /// both the smart-account sender (under 7702) and the Permit2
    /// `depositor`.
    pub secret_key: [u8; 32],
    pub eoa_address: Address,
    pub chain_id: u64,

    /// Skip `eth_estimateUserOperationGas` when `Some`. Set by
    /// [`crate::Client::fund_swap_gasless`] when
    /// [`crate::aa::AaConfig::gas_overrides`] is configured. See the
    /// field docs there for sizing guidance.
    pub gas_overrides: Option<GasOverrides>,
}

impl std::fmt::Debug for FundSwapInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FundSwapInputs")
            .field("coordinator_address", &self.coordinator_address)
            .field("source_token_address", &self.source_token_address)
            .field("source_amount", &self.source_amount)
            .field("lock_token_address", &self.lock_token_address)
            .field("preimage_hash", &self.preimage_hash)
            .field("claim_address", &self.claim_address)
            .field("timelock", &self.timelock)
            .field("calls", &self.calls)
            .field("calls_hash", &self.calls_hash)
            .field("entry_point", &self.entry_point)
            .field("delegation_target", &self.delegation_target)
            .field("secret_key", &"<redacted>")
            .field("eoa_address", &self.eoa_address)
            .field("chain_id", &self.chain_id)
            .field("gas_overrides", &self.gas_overrides)
            .finish()
    }
}

/// The clients + optional paymaster the orchestration needs. Passed by
/// reference so a single set can drive multiple swaps.
pub struct FundSwapClients<'a> {
    pub bundler: &'a BundlerClient,
    /// Node RPC for `eth_chainId`, `eth_getTransactionCount`, `EntryPoint.getNonce`,
    /// `eth_gasPrice`, and the transaction receipt that confirms what the bundler
    /// reports. Often the same URL as `bundler` (Alchemy co-locates them) but
    /// logically distinct.
    pub node: &'a RootProvider,
    /// Sponsorship paymaster — `None` when the sender pays its own gas
    /// (alto-against-Anvil dev setups, for instance). When `Some`, the
    /// orchestration runs the ERC-7677 `pm_*` dance and the userOp
    /// carries `paymasterAndData`; when `None`, those fields stay empty
    /// and EntryPoint pulls reimbursement from the sender's prefund.
    pub paymaster: Option<PaymasterRef<'a>>,
}

/// Paymaster client + its context, bundled so callers can't construct
/// a half-configured paymaster (URL without context or vice versa).
pub struct PaymasterRef<'a> {
    pub client: &'a PaymasterClient,
    pub context: Value,
}

/// What the orchestration returns once the userOp has been submitted
/// (and, where possible, mined).
#[derive(Debug, Clone)]
pub struct FundSwapReceipt {
    /// `userOpHash` of the submitted operation, computed locally and confirmed by the bundler.
    pub user_op_hash: B256,
    /// On-chain tx hash, once the bundler reported it and the node confirmed
    /// the operation ran in it within the polling window. `None` means neither
    /// happened yet; confirm a later bundler answer against the node before
    /// trusting it.
    pub transaction_hash: Option<B256>,
}

/// Drive the full gasless funding flow. See the module docstring for
/// the step-by-step.
pub async fn fund_swap(
    inputs: FundSwapInputs,
    clients: FundSwapClients<'_>,
) -> Result<FundSwapReceipt> {
    fund_swap_observed(inputs, clients, &|| Ok(()), &|_| Ok(())).await
}

/// Persist the reservation right before submission and the operation hash
/// immediately after it, before receipt polling.
pub(crate) async fn fund_swap_observed(
    inputs: FundSwapInputs,
    clients: FundSwapClients<'_>,
    reserve: &(dyn Fn() -> Result<()> + Send + Sync),
    submitted: &(dyn Fn(B256) -> Result<()> + Send + Sync),
) -> Result<FundSwapReceipt> {
    info!(
        eoa = %inputs.eoa_address,
        chain_id = inputs.chain_id,
        entry_point = %inputs.entry_point,
        delegation_target = %inputs.delegation_target,
        has_paymaster = clients.paymaster.is_some(),
        "fund_swap: starting"
    );

    let call_data = kernel::encode_execute_batch(&funding_calls(&inputs)?);

    publish_batch_observed(
        PublishInputs {
            secret_key: inputs.secret_key,
            eoa_address: inputs.eoa_address,
            chain_id: inputs.chain_id,
            entry_point: inputs.entry_point,
            delegation_target: inputs.delegation_target,
            call_data,
            gas_overrides: inputs.gas_overrides,
        },
        clients,
        reserve,
        submitted,
    )
    .await
}

/// Build the Permit2 allowance reset, the bounded approval and the signed funding
/// call for an atomic batch. Callers must validate the backend payload against their
/// swap and limits.
pub fn funding_calls(inputs: &FundSwapInputs) -> Result<Vec<Call>> {
    // 1. Permit2 witness signature.
    let (permit2_nonce, permit2_deadline) = generate_permit2_nonce_and_deadline();
    let permit2_witness_digest = permit2_digest(
        PermitWitnessParams {
            source_token: inputs.source_token_address,
            source_amount: inputs.source_amount,
            coordinator_address: inputs.coordinator_address,
            nonce: permit2_nonce,
            deadline: permit2_deadline,
            preimage_hash: inputs.preimage_hash,
            lock_token: inputs.lock_token_address,
            claim_address: inputs.claim_address,
            // Refund routes back through the coordinator itself — the
            // backend's `swap_and_lock_common.rs` documents this.
            refund_address: inputs.coordinator_address,
            timelock: U256::from(inputs.timelock),
            calls_hash: inputs.calls_hash,
        },
        inputs.chain_id,
    );
    let kernel_wrapped_digest =
        kernel::erc1271_wrapped_digest(permit2_witness_digest, inputs.eoa_address, inputs.chain_id);
    let permit2_sig_raw = signing::sign_hash(&inputs.secret_key, kernel_wrapped_digest)?;
    let permit2_signature = kernel::wrap_erc1271_signature(&sig_to_array(&permit2_sig_raw));

    // USDT reverts on a non-zero to non-zero approval, and wallet funding from the
    // same address can leave a residual allowance.
    Ok(vec![
        build_approve_permit2_call(inputs.source_token_address, U256::ZERO),
        build_approve_permit2_call(inputs.source_token_address, inputs.source_amount),
        build_execute_and_create_call(inputs, permit2_nonce, permit2_deadline, permit2_signature),
    ])
}

/// Fully encoded Kernel execution, signed by its EIP-7702 owner.
/// Call data may contain preimages; diagnostics omit both it and the key.
pub struct PublishInputs {
    pub secret_key: [u8; 32],
    pub eoa_address: Address,
    pub chain_id: u64,
    pub entry_point: Address,
    pub delegation_target: Address,
    pub call_data: Bytes,
    pub gas_overrides: Option<GasOverrides>,
}
impl std::fmt::Debug for PublishInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublishInputs")
            .field("eoa_address", &self.eoa_address)
            .field("chain_id", &self.chain_id)
            .finish_non_exhaustive()
    }
}

/// Publish a reviewed Kernel call batch with optional sponsorship. `reserve` runs
/// right before the operation leaves this process, so a failure in the nonce,
/// paymaster or estimation calls leaves no reservation behind; `submitted` must
/// persist the operation hash before the function starts receipt polling.
pub async fn publish_batch_observed(
    inputs: PublishInputs,
    clients: FundSwapClients<'_>,
    reserve: &(dyn Fn() -> Result<()> + Send + Sync),
    submitted: &(dyn Fn(B256) -> Result<()> + Send + Sync),
) -> Result<FundSwapReceipt> {
    if clients
        .node
        .get_chain_id()
        .await
        .map_err(|e| Error::Transport(e.to_string()))?
        != inputs.chain_id
    {
        return Err(Error::InvalidSwap(
            "UserOperation node chain mismatch".into(),
        ));
    }
    let ownership = signing::sign_hash(&inputs.secret_key, B256::ZERO)?;
    if ownership
        .recover_address_from_prehash(&B256::ZERO)
        .map_err(|e| Error::InvalidSigner(e.to_string()))?
        != inputs.eoa_address
    {
        return Err(Error::InvalidSigner(
            "UserOperation key does not match account".into(),
        ));
    }
    if inputs.entry_point.is_zero()
        || inputs.delegation_target.is_zero()
        || inputs.call_data.is_empty()
    {
        return Err(Error::InvalidSwap(
            "invalid UserOperation contracts or calls".into(),
        ));
    }
    // 3. Nonces.
    info!("fund_swap: fetching userOp nonce (EntryPoint.getNonce)");
    let userop_nonce =
        fetch_userop_nonce(clients.node, inputs.entry_point, inputs.eoa_address).await?;
    info!(%userop_nonce, "fund_swap: userOp nonce obtained");
    info!("fund_swap: fetching EOA tx nonce (eth_getTransactionCount)");
    let eoa_tx_nonce = clients
        .node
        .get_transaction_count(inputs.eoa_address)
        .pending()
        .await
        .map_err(|e| Error::Transport(format!("eth_getTransactionCount: {e}")))?;
    info!(eoa_tx_nonce, "fund_swap: EOA tx nonce obtained");

    // 4. EIP-7702 authorization.
    let auth = Authorization {
        chain_id: U256::from(inputs.chain_id),
        address: inputs.delegation_target,
        nonce: eoa_tx_nonce,
    };
    let auth_sig_raw = signing::sign_hash(&inputs.secret_key, auth.signature_hash())?;
    let signed_auth = SignedEip7702Authorization {
        chain_id: inputs.chain_id,
        address: inputs.delegation_target,
        nonce: eoa_tx_nonce,
        y_parity: u8::from(auth_sig_raw.v()),
        r: B256::from(auth_sig_raw.r().to_be_bytes::<32>()),
        s: B256::from(auth_sig_raw.s().to_be_bytes::<32>()),
    };

    // 5. Skeleton userOp.
    let mut userop = PackedUserOperation {
        sender: inputs.eoa_address,
        nonce: userop_nonce,
        factory: None,
        factory_data: None,
        call_data: inputs.call_data.clone(),
        call_gas_limit: U256::ZERO,
        verification_gas_limit: U256::ZERO,
        pre_verification_gas: U256::ZERO,
        max_fee_per_gas: U256::ZERO,
        max_priority_fee_per_gas: U256::ZERO,
        paymaster: None,
        paymaster_verification_gas_limit: None,
        paymaster_post_op_gas_limit: None,
        paymaster_data: None,
        signature: Bytes::from_static(&STUB_USEROP_SIGNATURE),
    };

    // 6. Paymaster stub. Skipped when there's no paymaster — the userOp's paymaster* fields stay
    //    None and EntryPoint will pull reimbursement from the sender's prefund at execution time.
    info!(
        skipped = clients.paymaster.is_none(),
        "fund_swap: paymaster stub data (pm_getPaymasterStubData)"
    );
    let stub_is_final = if let Some(pm) = clients.paymaster.as_ref() {
        let stub = pm
            .client
            .get_paymaster_stub_data(
                &userop,
                inputs.entry_point,
                inputs.chain_id,
                pm.context.clone(),
                Some(&signed_auth),
            )
            .await?;
        apply_paymaster(&mut userop, &stub);
        stub.is_final
    } else {
        // No stub call → no fields to apply. Treat as "final" so we
        // don't run a second `pm_*` call below.
        true
    };

    // 7. Gas price.
    info!("fund_swap: fetching gas price (eth_gasPrice)");
    let (max_fee, max_priority_fee) = fetch_gas_prices(clients.node).await?;
    userop.max_fee_per_gas = U256::from(max_fee);
    userop.max_priority_fee_per_gas = U256::from(max_priority_fee);
    info!(max_fee, max_priority_fee, "fund_swap: gas prices set");

    // 8. Gas limits — either bypass the bundler's estimation with caller-supplied values, or call
    //    `eth_estimateUserOperationGas`. The bypass exists because some bundlers (alto in
    //    particular) intermittently mis-simulate the userOp and return a misleading
    //    `TRANSFER_FROM_FAILED` from a perfectly valid batch — see the alto-estimation-flake
    //    feedback memo.
    if let Some(overrides) = inputs.gas_overrides {
        userop.call_gas_limit = U256::from(overrides.call_gas_limit);
        userop.verification_gas_limit = U256::from(overrides.verification_gas_limit);
        userop.pre_verification_gas = U256::from(overrides.pre_verification_gas);
        info!(
            call_gas_limit = %userop.call_gas_limit,
            verification_gas_limit = %userop.verification_gas_limit,
            pre_verification_gas = %userop.pre_verification_gas,
            "fund_swap: gas overrides applied (estimation skipped)"
        );
    } else {
        info!("fund_swap: estimating gas (eth_estimateUserOperationGas)");
        let gas = clients
            .bundler
            .estimate_user_operation_gas(&userop, inputs.entry_point, Some(&signed_auth))
            .await?;
        userop.call_gas_limit = gas.call_gas_limit;
        userop.verification_gas_limit = gas.verification_gas_limit;
        userop.pre_verification_gas = gas.pre_verification_gas;
        // A stub without paymaster limits leaves them to the estimate.
        if userop.paymaster.is_some() {
            if let Some(limit) = gas.paymaster_verification_gas_limit {
                userop.paymaster_verification_gas_limit = Some(limit);
            }
            if let Some(limit) = gas.paymaster_post_op_gas_limit {
                userop.paymaster_post_op_gas_limit = Some(limit);
            }
        }
        info!(
            call_gas_limit = %userop.call_gas_limit,
            verification_gas_limit = %userop.verification_gas_limit,
            pre_verification_gas = %userop.pre_verification_gas,
            "fund_swap: gas estimated"
        );
    }

    // 9. Final paymaster data (skip if no paymaster, or stub said it was final).
    if let Some(pm) = clients.paymaster.as_ref().filter(|_| !stub_is_final) {
        info!("fund_swap: final paymaster data (pm_getPaymasterData)");
        let final_pm = pm
            .client
            .get_paymaster_data(
                &userop,
                inputs.entry_point,
                inputs.chain_id,
                pm.context.clone(),
                Some(&signed_auth),
            )
            .await?;
        apply_paymaster(&mut userop, &final_pm);
    }

    resolve_paymaster_limits(&mut userop)?;

    // 10. Sign the userOpHash (EIP-191).
    let hash = user_op_hash(&userop, inputs.entry_point, inputs.chain_id);
    info!(user_op_hash = %hash, "fund_swap: signing userOpHash");
    let sig_raw = signing::sign_eip191_message(&inputs.secret_key, hash.as_slice())?;
    userop.signature = kernel::wrap_user_op_signature(&sig_to_array(&sig_raw));

    // 11. Submit.
    reserve()?;
    info!("fund_swap: submitting (eth_sendUserOperation)");
    let user_op_hash_out = clients
        .bundler
        .send_user_operation(&userop, inputs.entry_point, Some(&signed_auth))
        .await?;
    // The receipt lookups key on this hash, so it must be the one we signed, not the
    // bundler's answer. It is saved first because the operation may already be in flight.
    submitted(hash).map_err(|e| {
        Error::Storage(format!(
            "submitted user operation {hash}, but could not persist it: {e}"
        ))
    })?;
    if user_op_hash_out != hash {
        return Err(Error::InvalidSwap(format!(
            "bundler returned user operation hash {user_op_hash_out}, expected {hash}"
        )));
    }
    info!(user_op_hash = %hash, "fund_swap: submitted; polling receipt");

    // 12. Poll for the receipt.
    let transaction_hash =
        poll_receipt(clients.bundler, clients.node, inputs.entry_point, hash).await?;
    info!(?transaction_hash, "fund_swap: poll complete");

    Ok(FundSwapReceipt {
        user_op_hash: hash,
        transaction_hash,
    })
}

// ── helpers ────────────────────────────────────────────────────────────

/// Random 256-bit Permit2 nonce + an absolute deadline 30 minutes out.
fn generate_permit2_nonce_and_deadline() -> (U256, U256) {
    let mut nonce_bytes = [0u8; 32];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = U256::from_be_bytes(nonce_bytes);
    let deadline_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + PERMIT2_DEADLINE_HORIZON.as_secs();
    (nonce, U256::from(deadline_unix))
}

fn build_approve_permit2_call(source_token: Address, amount: U256) -> Call {
    let calldata = IERC20::approveCall {
        spender: PERMIT2_ADDRESS,
        amount,
    }
    .abi_encode();
    Call {
        target: source_token,
        value: U256::ZERO,
        callData: calldata.into(),
    }
}

fn build_execute_and_create_call(
    inputs: &FundSwapInputs,
    permit2_nonce: U256,
    permit2_deadline: U256,
    permit2_signature: Bytes,
) -> Call {
    let calldata = IHTLCCoordinator::executeAndCreateWithPermit2Call {
        calls: inputs.calls.clone(),
        preimageHash: inputs.preimage_hash,
        token: inputs.lock_token_address,
        claimAddress: inputs.claim_address,
        timelock: U256::from(inputs.timelock),
        depositor: inputs.eoa_address,
        permit: PermitTransferFrom {
            permitted: TokenPermissions {
                token: inputs.source_token_address,
                amount: inputs.source_amount,
            },
            nonce: permit2_nonce,
            deadline: permit2_deadline,
        },
        signature: permit2_signature,
    }
    .abi_encode();
    Call {
        target: inputs.coordinator_address,
        value: U256::ZERO,
        callData: calldata.into(),
    }
}

/// `EntryPoint.getNonce(sender, 0)` via raw `eth_call`. The 192-bit
/// nonce key is all-zero for the Kernel 7702 root validator path
/// (per the Phase 0 spike).
async fn fetch_userop_nonce(
    provider: &RootProvider,
    entry_point: Address,
    sender: Address,
) -> Result<U256> {
    let call = IEntryPoint::getNonceCall {
        sender,
        key: Uint::<192, 3>::ZERO,
    };
    let tx = TransactionRequest::default()
        .to(entry_point)
        .input(call.abi_encode().into());
    let returned: Bytes = provider
        .call(tx)
        .await
        .map_err(|e| Error::Transport(format!("EntryPoint.getNonce eth_call: {e}")))?;
    U256::abi_decode(&returned)
        .map_err(|e| Error::Decode(format!("EntryPoint.getNonce decode: {e}")))
}

/// `(max_fee_per_gas, max_priority_fee_per_gas)`. Uses `eth_gasPrice`
/// for the max-fee and a static floor for the priority fee — bundlers
/// reject `0` priority, and tuning beyond that is bundler-specific.
async fn fetch_gas_prices(provider: &RootProvider) -> Result<(u128, u128)> {
    let gas_price = provider
        .get_gas_price()
        .await
        .map_err(|e| Error::Transport(format!("eth_gasPrice: {e}")))?;
    // Bundlers only include an operation whose max fee covers the base fee
    // plus their own minimum priority fee at bundling time (Alchemy's rundler
    // on Arbitrum asks for ~1.1M wei). `eth_gasPrice` alone leaves no
    // headroom, so the operation is accepted, never bundled, then dropped.
    let max_priority = MIN_PRIORITY_FEE_WEI.max(gas_price / 10);
    let max_fee = gas_price.saturating_mul(2).saturating_add(max_priority);
    Ok((max_fee, max_priority))
}

/// Pin the paymaster gas limits before the operation is hashed and signed.
///
/// ERC-7677 leaves both limits optional in the stub, `pm_getPaymasterData`
/// never carries them, and Alchemy's `eth_estimateUserOperationGas` returns
/// only the verification limit. The packed layout has no "absent" value, so
/// whatever is unset here would be signed as zero implicitly. A zero
/// verification limit cannot validate, so it is an error. A zero post-op
/// limit is the documented meaning of a paymaster without a `postOp` hook
/// (viem packs a missing limit as zero as well), so it is recorded
/// explicitly and logged.
fn resolve_paymaster_limits(userop: &mut PackedUserOperation) -> Result<()> {
    if userop.paymaster.is_none() {
        return Ok(());
    }
    if userop.paymaster_verification_gas_limit.is_none() {
        return Err(Error::InvalidSwap(
            "paymaster verification gas limit unknown: the paymaster stub omitted it and \
             gas estimation did not supply it (estimation is skipped with gas overrides)"
                .into(),
        ));
    }
    if userop.paymaster_post_op_gas_limit.is_none() {
        info!("fund_swap: paymaster declared no post-op gas limit; signing it as zero");
        userop.paymaster_post_op_gas_limit = Some(U256::ZERO);
    }
    Ok(())
}

/// Apply paymaster fields onto an in-flight userOp. Used both for the
/// stub-data pass (gas estimation) and the final-data pass (submission).
fn apply_paymaster(
    userop: &mut PackedUserOperation,
    fields: &crate::aa::paymaster::PaymasterFields,
) {
    userop.paymaster = Some(fields.paymaster);
    // Limits the paymaster leaves out keep their current value: the stub's,
    // or the bundler's estimate once it has run (ERC-7677 makes them optional).
    if let Some(limit) = fields.paymaster_verification_gas_limit {
        userop.paymaster_verification_gas_limit = Some(limit);
    }
    if let Some(limit) = fields.paymaster_post_op_gas_limit {
        userop.paymaster_post_op_gas_limit = Some(limit);
    }
    userop.paymaster_data = Some(fields.paymaster_data.clone());
}

/// A single successful `UserOperationEvent` for `user_op_hash` from the pinned
/// EntryPoint proves the operation we signed ran in `tx`. The bundler alone
/// cannot vouch for this, so `receipt` must come from the node.
pub(crate) fn verify_user_operation_receipt(
    receipt: &EvmReceipt,
    entry_point: Address,
    user_op_hash: B256,
    tx: B256,
) -> Result<()> {
    let found = receipt
        .logs
        .iter()
        .filter(|log| log.address == entry_point)
        .filter_map(|log| {
            IEntryPoint::UserOperationEvent::decode_raw_log(log.topics.iter().copied(), &log.data)
                .ok()
        })
        .filter(|event| event.userOpHash == user_op_hash && event.success)
        .count();
    if receipt.transaction_hash != tx || !receipt.success || found != 1 {
        return Err(Error::InvalidSwap(format!(
            "bundler-reported transaction {tx} does not execute user operation {user_op_hash}"
        )));
    }
    Ok(())
}

/// Poll `eth_getUserOperationReceipt` until it lands or we exhaust the
/// attempt budget. Returns the tx hash if mined and confirmed by the node.
async fn poll_receipt(
    bundler: &BundlerClient,
    node: &RootProvider,
    entry_point: Address,
    user_op_hash: B256,
) -> Result<Option<B256>> {
    for attempt in 1..=RECEIPT_POLL_ATTEMPTS {
        match bundler.get_user_operation_receipt(user_op_hash).await {
            Ok(Some(receipt)) => {
                let tx = receipt.receipt.transaction_hash;
                info!(attempt, tx_hash = %tx, "fund_swap: receipt received");
                if !receipt.success {
                    return Err(Error::InvalidSwap(format!(
                        "user operation {user_op_hash} failed in transaction {tx}"
                    )));
                }
                match node.get_transaction_receipt(tx).await {
                    Ok(Some(mined)) => {
                        verify_user_operation_receipt(
                            &crate::evm::evm_receipt(&mined)?,
                            entry_point,
                            user_op_hash,
                            tx,
                        )?;
                        return Ok(Some(tx));
                    }
                    Ok(None) => {
                        tracing::debug!(attempt, %tx, "fund_swap: node has no receipt yet");
                    }
                    Err(e) => {
                        tracing::warn!(attempt, error = %e, "fund_swap: node receipt lookup errored");
                    }
                }
            }
            Ok(None) => {
                tracing::debug!(attempt, "fund_swap: receipt pending");
            }
            // Treat transient RPC errors as "still pending" — we'll
            // retry. Log them so a persistent failure (e.g. bundler
            // dropped the op) doesn't masquerade as a slow block.
            Err(e) => {
                tracing::warn!(attempt, error = %e, "fund_swap: receipt poll errored");
            }
        }
        tokio::time::sleep(RECEIPT_POLL_INTERVAL).await;
    }
    info!(
        attempts = RECEIPT_POLL_ATTEMPTS,
        "fund_swap: receipt poll exhausted without success"
    );
    Ok(None)
}

/// Convert an `alloy::primitives::Signature` to its raw `r ‖ s ‖ v`
/// 65-byte form — what Kernel V3.3's signature wrappers consume.
fn sig_to_array(sig: &alloy::primitives::Signature) -> [u8; 65] {
    let bytes = sig.as_bytes();
    debug_assert_eq!(bytes.len(), 65, "alloy Signature::as_bytes is always 65");
    let mut out = [0u8; 65];
    out.copy_from_slice(&bytes);
    out
}

/// Explicitly pinned AA contracts plus RPC and sponsorship settings.
#[derive(Clone, Debug)]
pub struct UserOpConfig {
    pub aa: crate::aa::AaConfig,
    pub entry_point: Address,
    pub delegation_target: Address,
}

pub async fn publish_with_config(
    secret_key: [u8; 32],
    account: Address,
    chain_id: u64,
    calls: &[Call],
    config: &UserOpConfig,
    reserve: &(dyn Fn() -> Result<()> + Send + Sync),
    submitted: &(dyn Fn(B256) -> Result<()> + Send + Sync),
) -> Result<FundSwapReceipt> {
    let bundler = BundlerClient::new(config.aa.bundler_url.clone(), config.aa.bundler_casing)?;
    let paymaster = config
        .aa
        .paymaster
        .as_ref()
        .map(|pm| PaymasterClient::new(pm.url.clone(), config.aa.bundler_casing))
        .transpose()?;
    let node = crate::evm::http_provider(config.aa.node_rpc_url.clone());
    publish_batch_observed(
        PublishInputs {
            secret_key,
            eoa_address: account,
            chain_id,
            entry_point: config.entry_point,
            delegation_target: config.delegation_target,
            call_data: kernel::encode_execute_batch(calls),
            gas_overrides: config.aa.gas_overrides,
        },
        FundSwapClients {
            bundler: &bundler,
            node: &node,
            paymaster: paymaster.as_ref().map(|client| PaymasterRef {
                client,
                context: config
                    .aa
                    .paymaster
                    .as_ref()
                    .map(|pm| pm.context.clone())
                    .unwrap_or(Value::Null),
            }),
        },
        reserve,
        submitted,
    )
    .await
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::evm::EvmLog;
    use serde_json::json;

    pub(crate) fn user_operation_log(
        entry_point: Address,
        user_op_hash: B256,
        success: bool,
    ) -> EvmLog {
        let data = IEntryPoint::UserOperationEvent {
            userOpHash: user_op_hash,
            sender: Address::repeat_byte(0x51),
            paymaster: Address::ZERO,
            nonce: U256::ZERO,
            success,
            actualGasCost: U256::from(1),
            actualGasUsed: U256::from(1),
        }
        .encode_log_data();
        EvmLog {
            address: entry_point,
            topics: data.topics().to_vec(),
            data: data.data,
        }
    }

    /// `eth_getTransactionReceipt` result for a successful transaction.
    pub(crate) fn node_receipt(tx: B256, logs: &[EvmLog]) -> Value {
        let logs: Vec<Value> = logs
            .iter()
            .enumerate()
            .map(|(index, log)| {
                json!({"address":log.address,"topics":log.topics,"data":log.data,"blockHash":B256::repeat_byte(0xbb),"blockNumber":"0xa","transactionHash":tx,"transactionIndex":"0x0","logIndex":format!("{index:#x}"),"removed":false})
            })
            .collect();
        json!({"transactionHash":tx,"transactionIndex":"0x0","blockHash":B256::repeat_byte(0xbb),"blockNumber":"0xa","from":Address::ZERO,"to":Address::ZERO,"cumulativeGasUsed":"0x1","gasUsed":"0x1","effectiveGasPrice":"0x1","contractAddress":null,"logs":logs,"logsBloom":format!("0x{}","00".repeat(256)),"type":"0x2","status":"0x1"})
    }

    /// The hash of the operation in an `eth_sendUserOperation` request, as the SDK signs it.
    pub(crate) fn submitted_user_op_hash(body: &Value, chain_id: u64) -> B256 {
        let op: PackedUserOperation = serde_json::from_value(body["params"][0].clone()).unwrap();
        let entry_point: Address = serde_json::from_value(body["params"][1].clone()).unwrap();
        user_op_hash(&op, entry_point, chain_id)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::aa::bundler::BundlerCasing;
    use crate::evm::EvmLog;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;

    #[tokio::test]
    async fn failed_user_operation_is_not_reported_as_success() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::method("POST")).respond_with(|request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":{"success":false,"receipt":{"transactionHash":B256::repeat_byte(1)}}}))
        }).expect(1).mount(&server).await;
        let bundler =
            BundlerClient::new(server.uri().parse().unwrap(), BundlerCasing::CamelCase).unwrap();
        let node = crate::evm::http_provider(server.uri().parse().unwrap());
        assert!(matches!(
            poll_receipt(
                &bundler,
                &node,
                Address::repeat_byte(12),
                B256::repeat_byte(2)
            )
            .await,
            Err(Error::InvalidSwap(_))
        ));
    }

    #[tokio::test]
    async fn bundler_receipt_is_accepted_only_when_the_node_shows_the_operation() {
        let entry_point = Address::repeat_byte(12);
        let operation = B256::repeat_byte(2);
        let tx = B256::repeat_byte(1);
        for (logs, accepted) in [
            (vec![], false),
            (
                vec![user_operation_log(entry_point, B256::repeat_byte(3), true)],
                false,
            ),
            (vec![user_operation_log(entry_point, operation, true)], true),
        ] {
            let server = MockServer::start().await;
            let receipt = node_receipt(tx, &logs);
            Mock::given(wiremock::matchers::method("POST"))
                .respond_with(move |request: &wiremock::Request| {
                    let body: Value = request.body_json().unwrap();
                    let result = match body["method"].as_str().unwrap() {
                        "eth_getUserOperationReceipt" => {
                            json!({"success":true,"receipt":{"transactionHash":tx}})
                        }
                        "eth_getTransactionReceipt" => receipt.clone(),
                        other => panic!("unexpected RPC {other}"),
                    };
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
                })
                .mount(&server)
                .await;
            let bundler =
                BundlerClient::new(server.uri().parse().unwrap(), BundlerCasing::CamelCase)
                    .unwrap();
            let node = crate::evm::http_provider(server.uri().parse().unwrap());
            let polled = poll_receipt(&bundler, &node, entry_point, operation).await;
            if accepted {
                assert_eq!(polled.unwrap(), Some(tx));
            } else {
                assert!(matches!(polled, Err(Error::InvalidSwap(_))), "{polled:?}");
            }
        }
    }

    #[tokio::test]
    async fn bundler_hash_that_differs_from_the_signed_operation_is_refused_after_saving_ours() {
        use std::sync::Arc;
        use std::sync::Mutex;
        let server = MockServer::start().await;
        let signed = Arc::new(Mutex::new(None));
        let observed = signed.clone();
        Mock::given(wiremock::matchers::method("POST")).respond_with(move |request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            let result = match body["method"].as_str().unwrap() {
                "eth_chainId" => json!("0x89"),
                "eth_getTransactionCount" => json!("0x3"),
                "eth_call" => json!(format!("0x{:064x}", 0)),
                "eth_gasPrice" => json!("0x100000"),
                "eth_estimateUserOperationGas" => json!({"callGasLimit":"0x70000","verificationGasLimit":"0x20000","preVerificationGas":"0x10000"}),
                "eth_sendUserOperation" => {
                    *observed.lock().unwrap() = Some(submitted_user_op_hash(&body, 137));
                    json!(B256::repeat_byte(7))
                }
                other => panic!("unexpected RPC {other}"),
            };
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
        }).mount(&server).await;
        let bundler =
            BundlerClient::new(server.uri().parse().unwrap(), BundlerCasing::CamelCase).unwrap();
        let node = crate::evm::http_provider(server.uri().parse().unwrap());
        let secret_key = [1u8; 32];
        let saved = Mutex::new(Vec::new());
        let result = publish_batch_observed(
            PublishInputs {
                secret_key,
                eoa_address: signing::sign_hash(&secret_key, B256::ZERO)
                    .unwrap()
                    .recover_address_from_prehash(&B256::ZERO)
                    .unwrap(),
                chain_id: 137,
                entry_point: Address::repeat_byte(12),
                delegation_target: Address::repeat_byte(13),
                call_data: Bytes::from_static(&[1]),
                gas_overrides: None,
            },
            FundSwapClients {
                bundler: &bundler,
                node: &node,
                paymaster: None,
            },
            &|| Ok(()),
            &|hash| {
                saved.lock().unwrap().push(hash);
                Ok(())
            },
        )
        .await;
        assert!(
            matches!(&result, Err(Error::InvalidSwap(m)) if m.starts_with("bundler returned user operation hash")),
            "{result:?}"
        );
        let signed = signed.lock().unwrap().unwrap();
        assert_ne!(signed, B256::repeat_byte(7));
        assert_eq!(*saved.lock().unwrap(), vec![signed]);
    }

    #[test]
    fn user_operation_receipt_needs_exactly_one_successful_event_from_the_entry_point() {
        let entry_point = Address::repeat_byte(12);
        let operation = B256::repeat_byte(2);
        let tx = B256::repeat_byte(1);
        let receipt = |logs: Vec<EvmLog>| EvmReceipt {
            transaction_hash: tx,
            success: true,
            block_number: 10,
            logs,
        };
        // topic0 of the event the deployed EntryPoint v0.7 emits
        assert_eq!(
            IEntryPoint::UserOperationEvent::SIGNATURE_HASH,
            alloy::primitives::b256!(
                "49628fd1471006c1482da88028e9ce4dbb080b815c9b0344d39e5a8e6ec1419f"
            )
        );
        let event = user_operation_log(entry_point, operation, true);
        assert!(
            verify_user_operation_receipt(
                &receipt(vec![event.clone()]),
                entry_point,
                operation,
                tx
            )
            .is_ok()
        );
        let mut foreign_emitter = event.clone();
        foreign_emitter.address = Address::repeat_byte(13);
        for logs in [
            vec![],
            vec![foreign_emitter],
            vec![user_operation_log(entry_point, B256::repeat_byte(3), true)],
            vec![user_operation_log(entry_point, operation, false)],
            vec![event.clone(), event.clone()],
        ] {
            assert!(
                verify_user_operation_receipt(&receipt(logs), entry_point, operation, tx).is_err()
            );
        }
        assert!(
            verify_user_operation_receipt(
                &receipt(vec![event.clone()]),
                entry_point,
                operation,
                B256::repeat_byte(4)
            )
            .is_err()
        );
        let mut reverted = receipt(vec![event]);
        reverted.success = false;
        assert!(verify_user_operation_receipt(&reverted, entry_point, operation, tx).is_err());
    }

    fn sponsored_op() -> PackedUserOperation {
        PackedUserOperation {
            sender: Address::ZERO,
            nonce: U256::ZERO,
            factory: None,
            factory_data: None,
            call_data: Bytes::new(),
            call_gas_limit: U256::from(1u64),
            verification_gas_limit: U256::from(1u64),
            pre_verification_gas: U256::from(1u64),
            max_fee_per_gas: U256::from(1u64),
            max_priority_fee_per_gas: U256::from(1u64),
            paymaster: Some(Address::repeat_byte(0x44)),
            paymaster_verification_gas_limit: Some(U256::from(50_000u64)),
            paymaster_post_op_gas_limit: None,
            paymaster_data: Some(Bytes::from_static(&[0xca, 0xfe])),
            signature: Bytes::new(),
        }
    }

    /// A missing post-op limit is a paymaster without `postOp`: signed as zero.
    #[test]
    fn missing_post_op_limit_is_pinned_to_zero() {
        let mut op = sponsored_op();
        resolve_paymaster_limits(&mut op).unwrap();
        assert_eq!(op.paymaster_post_op_gas_limit, Some(U256::ZERO));
        assert_eq!(
            op.paymaster_verification_gas_limit,
            Some(U256::from(50_000u64))
        );
    }

    /// Limits the paymaster did supply are kept as they are.
    #[test]
    fn supplied_limits_are_kept() {
        let mut op = sponsored_op();
        op.paymaster_post_op_gas_limit = Some(U256::from(7u64));
        resolve_paymaster_limits(&mut op).unwrap();
        assert_eq!(op.paymaster_post_op_gas_limit, Some(U256::from(7u64)));
    }

    /// Nothing supplied the verification limit (stub omitted it and estimation
    /// was skipped): refuse rather than sign a zero limit.
    #[test]
    fn missing_verification_limit_is_an_error() {
        let mut op = sponsored_op();
        op.paymaster_verification_gas_limit = None;
        let err = resolve_paymaster_limits(&mut op).unwrap_err();
        assert!(
            matches!(err, Error::InvalidSwap(ref m) if m.contains("verification gas limit")),
            "{err:?}"
        );
    }

    /// Without a paymaster there is nothing to resolve.
    #[test]
    fn no_paymaster_is_untouched() {
        let mut op = sponsored_op();
        op.paymaster = None;
        op.paymaster_verification_gas_limit = None;
        resolve_paymaster_limits(&mut op).unwrap();
        assert_eq!(op.paymaster_post_op_gas_limit, None);
    }
}
