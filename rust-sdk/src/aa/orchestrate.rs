//! End-to-end gasless-funding orchestration: takes a swap's backend
//! calldata + the depositor's per-swap key, drives the full ERC-4337 +
//! EIP-7702 flow against a bundler + paymaster + node RPC, and returns
//! the userOpHash plus (when available) the on-chain transaction hash.
//!
//! Step ordering:
//!
//! 1. Build the Permit2 witness signature (Kernel-V3.3-wrapped under 7702), so the signature can be
//!    embedded in the coordinator call.
//! 2. Encode the outer batch — `USDC.approve(Permit2, max)` +
//!    `HTLCCoordinator.executeAndCreateWithPermit2(..., permit2_sig)` — wrap it in Kernel's
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
//! 11. `eth_sendUserOperation` (with auth) → returns userOpHash.
//! 12. Poll `eth_getUserOperationReceipt` until mined (bounded).
//!
//! All RPC calls inherit the [`BundlerCasing`] chosen on the clients —
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
#[derive(Debug, Clone)]
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

/// The clients + optional paymaster the orchestration needs. Passed by
/// reference so a single set can drive multiple swaps.
pub struct FundSwapClients<'a> {
    pub bundler: &'a BundlerClient,
    /// Node RPC for `eth_getTransactionCount`, `EntryPoint.getNonce`,
    /// and `eth_gasPrice`. Often the same URL as `bundler` (Alchemy
    /// co-locates them) but logically distinct.
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
    /// `userOpHash` the bundler computed on submission.
    pub user_op_hash: B256,
    /// On-chain tx hash, if the receipt arrived within the polling
    /// window. `None` means submitted-but-not-yet-mined — the caller
    /// can poll `BundlerClient::get_user_operation_receipt` later.
    pub transaction_hash: Option<B256>,
}

/// Drive the full gasless funding flow. See the module docstring for
/// the step-by-step.
pub async fn fund_swap(
    inputs: FundSwapInputs,
    clients: FundSwapClients<'_>,
) -> Result<FundSwapReceipt> {
    info!(
        eoa = %inputs.eoa_address,
        chain_id = inputs.chain_id,
        entry_point = %inputs.entry_point,
        delegation_target = %inputs.delegation_target,
        has_paymaster = clients.paymaster.is_some(),
        "fund_swap: starting"
    );

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

    // 2. Outer batch: approve(Permit2, max) + executeAndCreateWithPermit2.
    let approve_call = build_approve_permit2_call(inputs.source_token_address);
    let execute_and_create_call =
        build_execute_and_create_call(&inputs, permit2_nonce, permit2_deadline, permit2_signature);
    let call_data = kernel::encode_execute_batch(&[approve_call, execute_and_create_call]);

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
        call_data,
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

    // 10. Sign the userOpHash (EIP-191).
    let hash = user_op_hash(&userop, inputs.entry_point, inputs.chain_id);
    info!(user_op_hash = %hash, "fund_swap: signing userOpHash");
    let sig_raw = signing::sign_eip191_message(&inputs.secret_key, hash.as_slice())?;
    userop.signature = kernel::wrap_user_op_signature(&sig_to_array(&sig_raw));

    // 11. Submit.
    info!("fund_swap: submitting (eth_sendUserOperation)");
    let user_op_hash_out = clients
        .bundler
        .send_user_operation(&userop, inputs.entry_point, Some(&signed_auth))
        .await?;
    info!(user_op_hash = %user_op_hash_out, "fund_swap: submitted; polling receipt");

    // 12. Poll for the receipt.
    let transaction_hash = poll_receipt(clients.bundler, user_op_hash_out).await;
    info!(?transaction_hash, "fund_swap: poll complete");

    Ok(FundSwapReceipt {
        user_op_hash: user_op_hash_out,
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

fn build_approve_permit2_call(source_token: Address) -> Call {
    let calldata = IERC20::approveCall {
        spender: PERMIT2_ADDRESS,
        amount: U256::MAX,
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
    let max_priority = MIN_PRIORITY_FEE_WEI;
    let max_fee = gas_price.max(max_priority);
    Ok((max_fee, max_priority))
}

/// Apply paymaster fields onto an in-flight userOp. Used both for the
/// stub-data pass (gas estimation) and the final-data pass (submission).
fn apply_paymaster(
    userop: &mut PackedUserOperation,
    fields: &crate::aa::paymaster::PaymasterFields,
) {
    userop.paymaster = Some(fields.paymaster);
    userop.paymaster_verification_gas_limit = Some(fields.paymaster_verification_gas_limit);
    userop.paymaster_post_op_gas_limit = Some(fields.paymaster_post_op_gas_limit);
    userop.paymaster_data = Some(fields.paymaster_data.clone());
}

/// Poll `eth_getUserOperationReceipt` until it lands or we exhaust the
/// attempt budget. Returns the tx hash if mined.
async fn poll_receipt(bundler: &BundlerClient, user_op_hash: B256) -> Option<B256> {
    for attempt in 1..=RECEIPT_POLL_ATTEMPTS {
        match bundler.get_user_operation_receipt(user_op_hash).await {
            Ok(Some(receipt)) => {
                info!(
                    attempt,
                    tx_hash = %receipt.receipt.transaction_hash,
                    "fund_swap: receipt received"
                );
                return Some(receipt.receipt.transaction_hash);
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
    None
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
