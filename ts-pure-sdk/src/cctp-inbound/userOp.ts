/**
 * Compose the `executeBatch` calldata for a CCTP-inbound UserOperation.
 *
 * The smart account's batch atomically runs:
 *   1. `MessageTransmitter.receiveMessage(message, attestation)` — mints
 *      USDC to the smart account.
 *   2. `USDC.approve(Permit2, max)` — idempotent.
 *   3. `HTLCCoordinator.executeAndCreateWithPermit2(...)` — uses an
 *      ERC-1271 signature from the smart account as `depositor`.
 *
 * All three calls execute with `msg.sender = smartAccount`, so CCTPv2's
 * `destinationCaller` gate (pinned to the same address on the burn) is
 * satisfied.
 */

import type { Address, Hex, TypedDataDomain } from "viem";
import {
  encodeAbiParameters,
  encodeFunctionData,
  isErc6492Signature,
  maxUint256,
  parseAbi,
  parseErc6492Signature,
} from "viem";
import { buildPermit2TypedData } from "../evm/coordinator.js";

/** MessageTransmitter v2 — same on every CCTPv2 EVM chain. */
const MESSAGE_TRANSMITTER_V2 =
  "0x81D40F21F12A8F0E3252Bccb954D722d4c464B64" as const;

/** Canonical Permit2 deployment — same on all EVM chains. */
const PERMIT2_ADDRESS = "0x000000000022D473030F116dDEE9F6B43aC78BA3" as const;

const MESSAGE_TRANSMITTER_ABI = parseAbi([
  "function receiveMessage(bytes message, bytes attestation) external returns (bool)",
]);

const ERC20_APPROVE_ABI = parseAbi([
  "function approve(address spender, uint256 amount) external returns (bool)",
]);

const HTLC_COORDINATOR_ABI = parseAbi([
  "struct TokenPermissions { address token; uint256 amount; }",
  "struct PermitTransferFrom { TokenPermissions permitted; uint256 nonce; uint256 deadline; }",
  "struct Call { address target; uint256 value; bytes callData; }",
  "function executeAndCreateWithPermit2(Call[] calls, bytes32 preimageHash, address token, address claimAddress, uint256 timelock, address depositor, PermitTransferFrom permit, bytes signature) external",
]);

/**
 * Shape returned by the backend's
 * `GET /v1/swap/:id/swap-and-lock-calldata-userop` endpoint.
 * Field names match the REST JSON (snake_case).
 */
export interface UseropCalldataResponse {
  coordinator_address: string;
  permit2_address: string;
  source_token_address: string;
  source_amount: string;
  lock_token_address: string;
  preimage_hash: string;
  claim_address: string;
  timelock: number;
  calls: Array<{ target: string; value: string; call_data: string }>;
  calls_hash: string;
  relay_fee?: string;
  aa: {
    entry_point: string;
    /** Kernel implementation the depositor EOA delegates to via EIP-7702. */
    delegation_target: string;
  };
}

/**
 * Sign arbitrary EIP-712 typed data with the smart account.
 *
 * For Kernel this wraps the hash with its "message marker" before
 * ecrecover, so the resulting sig passes ERC-1271 verification on the
 * deployed account. Pass `kernelAccount.signTypedData` directly.
 */
// biome-ignore lint/suspicious/noExplicitAny: typed-data shape is library-specific
export type SignTypedDataFn = (args: any) => Promise<Hex>;

export interface BuildCctpInboundBatchParams {
  /** Calldata payload from the backend. */
  server: UseropCalldataResponse;
  /** Deterministic smart-account address that owns the HTLC deposit. */
  smartAccountAddress: Address;
  /** Produces an ERC-1271-compatible Permit2 signature via the smart account. */
  signTypedData: SignTypedDataFn;
  /** Raw IRIS message bytes (hex, 0x-prefixed). */
  cctpMessage: Hex;
  /** Raw IRIS attestation bytes (hex, 0x-prefixed). */
  cctpAttestation: Hex;
  /** Settlement chain id — used as the Permit2 domain chain id. */
  chainId: number;
  /** Whether USDC is already at `smartAccountAddress` (skip receiveMessage). */
  skipReceiveMessage?: boolean;
}

/** A single (target, data, value) call the smart account will execute. */
export interface BatchCall {
  to: Address;
  data: Hex;
  value: bigint;
}

export interface BuiltBatch {
  calls: BatchCall[];
  /** The Permit2 nonce used in the witness; useful to log / surface. */
  permit2Nonce: bigint;
  /** Deadline embedded in the Permit2 signature (unix seconds). */
  permit2Deadline: bigint;
}

/**
 * Returns the list of calls the smart account's `executeBatch` will run.
 * The Permit2 signature is produced via the Kernel smart account's
 * `signTypedData` — Kernel wraps the hash before signing so the sig
 * passes its own ERC-1271 verification once the account is deployed.
 */
export async function buildCctpInboundBatch(
  params: BuildCctpInboundBatchParams,
): Promise<BuiltBatch> {
  const {
    server,
    smartAccountAddress,
    signTypedData,
    cctpMessage,
    cctpAttestation,
    chainId,
    skipReceiveMessage,
  } = params;

  const calls: BatchCall[] = [];

  // 1. receiveMessage — skip if USDC already landed (front-runner / retry).
  if (!skipReceiveMessage) {
    const receiveMessageData = encodeFunctionData({
      abi: MESSAGE_TRANSMITTER_ABI,
      functionName: "receiveMessage",
      args: [cctpMessage, cctpAttestation],
    });
    calls.push({
      to: MESSAGE_TRANSMITTER_V2 as Address,
      data: receiveMessageData,
      value: 0n,
    });
  }

  // 2. USDC.approve(Permit2, max) — idempotent; lets Permit2 pull from us.
  const approveData = encodeFunctionData({
    abi: ERC20_APPROVE_ABI,
    functionName: "approve",
    args: [PERMIT2_ADDRESS as Address, maxUint256],
  });
  calls.push({
    to: server.source_token_address as Address,
    data: approveData,
    value: 0n,
  });

  // 3. Permit2 witness signature, produced via the smart account.
  //    Kernel's signTypedData applies its message-marker wrap so the
  //    resulting sig passes ERC-1271 when Permit2 calls back into the
  //    (by then-deployed) account.
  const nonceBytes = new Uint8Array(32);
  crypto.getRandomValues(nonceBytes);
  const permit2Nonce = BigInt(
    `0x${Array.from(nonceBytes)
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("")}`,
  );
  const permit2Deadline = BigInt(Math.floor(Date.now() / 1000) + 30 * 60);
  const sourceAmount = BigInt(server.source_amount);

  const typedData = buildPermit2TypedData({
    chainId,
    coordinatorAddress: server.coordinator_address,
    sourceToken: server.source_token_address,
    sourceAmount,
    preimageHash: server.preimage_hash,
    lockToken: server.lock_token_address,
    claimAddress: server.claim_address,
    refundAddress: server.coordinator_address,
    timelock: server.timelock,
    callsHash: server.calls_hash,
    nonce: permit2Nonce,
    deadline: permit2Deadline,
  });

  const rawSignature = (await signTypedData({
    domain: typedData.domain as TypedDataDomain,
    types: typedData.types,
    primaryType: typedData.primaryType,
    message: typedData.message,
  })) as Hex;

  // viem's `toSmartAccount` auto-wraps `signTypedData` output in ERC-6492
  // whenever the account's factory/factoryData are non-empty. Permit2 is
  // not ERC-6492-aware, so strip the wrapper and pass the raw Kernel
  // signature. The account will be deployed by factoryData during the
  // UserOp *before* Permit2 verifies, so Permit2 takes the ERC-1271
  // path and Kernel's `isValidSignature` accepts the raw sig.
  const permit2Signature = isErc6492Signature(rawSignature)
    ? parseErc6492Signature(rawSignature).signature
    : rawSignature;

  // 4. executeAndCreateWithPermit2
  const dexCalls = server.calls.map((c) => ({
    target: c.target as Address,
    value: BigInt(c.value),
    callData: c.call_data as Hex,
  }));

  const executeData = encodeFunctionData({
    abi: HTLC_COORDINATOR_ABI,
    functionName: "executeAndCreateWithPermit2",
    args: [
      dexCalls,
      server.preimage_hash as Hex,
      server.lock_token_address as Address,
      server.claim_address as Address,
      BigInt(server.timelock),
      smartAccountAddress,
      {
        permitted: {
          token: server.source_token_address as Address,
          amount: sourceAmount,
        },
        nonce: permit2Nonce,
        deadline: permit2Deadline,
      },
      permit2Signature,
    ],
  });
  calls.push({
    to: server.coordinator_address as Address,
    data: executeData,
    value: 0n,
  });

  return {
    calls,
    permit2Nonce,
    permit2Deadline,
  };
}

/**
 * Convert an EVM address (20 bytes) into the bytes32 form CCTPv2's
 * `depositForBurn` expects for `mintRecipient` and `destinationCaller`.
 */
export function addressToBytes32Hex(address: Address): Hex {
  return encodeAbiParameters([{ type: "address" }], [address]) as Hex;
}
