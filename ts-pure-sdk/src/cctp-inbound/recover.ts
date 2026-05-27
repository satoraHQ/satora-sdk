/**
 * `recoverCctpInbound` — sweep USDC stranded in the user's smart account
 * after a CCTP burn whose HTLC settlement never completed.
 *
 * Two failure modes are handled:
 *   - User burned on the source chain but never called `receiveMessage`
 *     on Arbitrum (smart account holds 0 USDC; the message is unconsumed).
 *   - User called `receiveMessage` but the HTLC fund step reverted
 *     (smart account already holds the minted USDC).
 *
 * Strategy: at most two paymaster-sponsored UserOps.
 *   1. If `MessageTransmitter.usedNonces(nonce) == 0`: send a UserOp that
 *      calls `receiveMessage(message, attestation)`. The smart account
 *      remains the `destinationCaller`, so only it can submit.
 *   2. Read the smart account's USDC balance, then send a UserOp that
 *      calls `USDC.transfer(recipient, balance)`.
 *
 * The two-UserOp split avoids decoding the CCTP message body to predict
 * the post-mint balance — `balanceOf` between the calls is authoritative.
 */

import type { Address, Chain, Hex } from "viem";
import {
  createPublicClient,
  encodeFunctionData,
  erc20Abi,
  http,
  parseAbi,
} from "viem";
import { arbitrum } from "viem/chains";
import { fetchAttestation } from "../cctp/attestation.js";
import {
  type CctpChainName,
  IRIS_API_MAINNET,
  MESSAGE_TRANSMITTER_V2,
  USDC_ADDRESSES,
} from "../cctp/constants.js";
import type { EvmSigner } from "../evm/wallet.js";
import { createSwapSmartAccountClient } from "./smartAccount.js";
import type { AaConfig } from "./types.js";

const MESSAGE_TRANSMITTER_ABI = parseAbi([
  "function receiveMessage(bytes message, bytes attestation) external returns (bool)",
]);

const USED_NONCES_ABI = [
  {
    type: "function",
    name: "usedNonces",
    stateMutability: "view",
    inputs: [{ name: "nonce", type: "bytes32" }],
    outputs: [{ name: "", type: "uint256" }],
  },
] as const;

/**
 * CCTPv2 message header layout: the per-burn nonce sits at bytes 12..44.
 * As a 0x-prefixed hex string, that's chars [26, 90).
 */
function extractNonce(cctpMessage: Hex): Hex {
  return `0x${cctpMessage.slice(26, 26 + 64)}` as Hex;
}

export interface CheckCctpRecoverableParams {
  /** Smart-account address that owned the burn's `destinationCaller`. Under EIP-7702 this is the user's EOA. */
  smartAccountAddress: Address;
  /** Source-chain `depositForBurn` tx hash. */
  burnTxHash: Hex;
  /** SDK chain name the burn originated on. */
  sourceChain: CctpChainName;
  /** Settlement chain. Defaults to Arbitrum. */
  chain?: Chain;
  /** IRIS API base URL. Defaults to mainnet. */
  irisApiUrl?: string;
  /** Optional abort signal — propagates to the attestation poll. */
  signal?: AbortSignal;
  /**
   * Max ms to wait for IRIS attestation before bailing out and assuming
   * "recoverable" (conservative — better to surface the UI than hide it).
   * Defaults to 10_000.
   */
  attestationTimeoutMs?: number;
}

export interface CheckCctpRecoverableResult {
  /**
   * `true` when the recovery flow has work to do: either USDC sits at
   * the smart account ready to sweep, or the burn message hasn't been
   * consumed on the destination yet (so a sweep would mint and forward).
   * `false` only when the smart account is empty AND the burn nonce is
   * already used — i.e. claimed and swept.
   */
  recoverable: boolean;
  /** Current USDC balance at the smart account on the settlement chain. */
  balance: bigint;
  /**
   * Whether `MessageTransmitter.usedNonces(nonce)` returned non-zero.
   * `undefined` when we couldn't determine it (attestation timeout).
   */
  alreadyClaimed: boolean | undefined;
}

/**
 * Cheap on-chain check to decide whether the recovery UI should be
 * surfaced. Two reads on the settlement chain (balance + usedNonces)
 * plus one IRIS lookup for the burn's nonce. No signatures, no UserOps.
 *
 * Conservative on failure: if IRIS times out or rejects, returns
 * `recoverable: true` so the UI errs on the side of showing the option
 * rather than silently hiding stuck funds.
 */
export async function checkCctpRecoverable(
  context: RecoverCctpInboundContext,
  params: CheckCctpRecoverableParams,
): Promise<CheckCctpRecoverableResult> {
  const {
    smartAccountAddress,
    burnTxHash,
    sourceChain,
    chain = arbitrum,
    irisApiUrl = IRIS_API_MAINNET,
    signal,
    attestationTimeoutMs = 10_000,
  } = params;

  const publicClient = createPublicClient({
    chain,
    transport: http(context.aa.bundlerUrl),
  });

  const balance = (await publicClient.readContract({
    address: USDC_ADDRESSES.Arbitrum as Address,
    abi: erc20Abi,
    functionName: "balanceOf",
    args: [smartAccountAddress],
  })) as bigint;

  // Balance alone is enough to declare recoverable — no need to bother
  // IRIS / usedNonces. The sweep UserOp will handle it.
  if (balance > 0n) {
    return { recoverable: true, balance, alreadyClaimed: undefined };
  }

  // Smart account is empty; recovery only makes sense if the burn
  // message hasn't been consumed yet. Fetch the attestation to learn
  // the nonce, with a short timeout so a flaky IRIS doesn't hide the UI.
  let alreadyClaimed: boolean | undefined;
  try {
    const fetched = await fetchAttestation({
      sourceChain,
      txHash: burnTxHash,
      irisApiUrl,
      signal,
      timeoutMs: attestationTimeoutMs,
      pollIntervalMs: Math.min(attestationTimeoutMs, 5_000),
    });
    const nonce = extractNonce(fetched.message as Hex);
    const usedNonce = (await publicClient.readContract({
      address: MESSAGE_TRANSMITTER_V2 as Address,
      abi: USED_NONCES_ABI,
      functionName: "usedNonces",
      args: [nonce],
    })) as bigint;
    alreadyClaimed = usedNonce !== 0n;
  } catch {
    // IRIS not ready or unreachable — be conservative.
    return { recoverable: true, balance, alreadyClaimed: undefined };
  }

  // Empty smart account + nonce consumed = already swept. Nothing to do.
  if (alreadyClaimed) {
    return { recoverable: false, balance, alreadyClaimed };
  }
  // Empty smart account + nonce unused = user still needs to claim.
  return { recoverable: true, balance, alreadyClaimed };
}

export type RecoveryProgress =
  | { phase: "attestation" }
  | { phase: "receiving"; userOpHash: Hex }
  | { phase: "sweeping"; userOpHash: Hex; amount: bigint }
  | {
      phase: "done";
      receiveUserOpHash?: Hex;
      sweepUserOpHash: Hex;
      sweepTxHash?: Hex;
      recoveredAmount: bigint;
    };

export interface RecoverCctpInboundParams {
  /** Kernel-account owner; must match the original burn's `destinationCaller`. */
  signer: EvmSigner;
  /** Source-chain `depositForBurn` tx hash. */
  burnTxHash: Hex;
  /** SDK chain name the burn originated on (e.g. "HyperEVM"). */
  sourceChain: CctpChainName;
  /** EVM address that should receive the recovered USDC on the settlement chain. */
  recipient: Address;
  /** Settlement chain. Defaults to Arbitrum. */
  chain?: Chain;
  /** IRIS API base URL. Defaults to mainnet. */
  irisApiUrl?: string;
  /**
   * Pre-fetched attestation, if available. Skips the IRIS poll when
   * provided — useful when the caller has the burn cached from an
   * earlier session.
   */
  attestation?: { message: Hex; attestation: Hex };
  /** Progress callback for UI updates. */
  onProgress?: (step: RecoveryProgress) => void;
  /** Abort signal — propagates to the attestation poll. */
  signal?: AbortSignal;
}

export interface RecoverCctpInboundResult {
  /** UserOp that called `receiveMessage`; omitted when it was already consumed. */
  receiveUserOpHash?: Hex;
  /** UserOp that swept USDC to `recipient`. */
  sweepUserOpHash: Hex;
  /** On-chain tx hash of the sweep UserOp's bundle. */
  sweepTxHash?: Hex;
  /** Smart-account address the funds were swept from. */
  smartAccountAddress: Address;
  /** USDC amount transferred to `recipient`, in smallest units (6 decimals). */
  recoveredAmount: bigint;
}

export interface RecoverCctpInboundContext {
  aa: AaConfig;
}

/**
 * Recover USDC from a stalled CCTP-inbound flow by sweeping the smart
 * account's balance to a user-provided address. Paymaster-sponsored,
 * so the smart account needs no ETH.
 */
export async function recoverCctpInbound(
  context: RecoverCctpInboundContext,
  params: RecoverCctpInboundParams,
): Promise<RecoverCctpInboundResult> {
  const {
    signer,
    burnTxHash,
    sourceChain,
    recipient,
    chain = arbitrum,
    irisApiUrl = IRIS_API_MAINNET,
    onProgress,
    signal,
  } = params;

  const { client: aaClient, accountAddress } =
    await createSwapSmartAccountClient({ signer, aa: context.aa, chain });

  const publicClient = createPublicClient({
    chain,
    transport: http(context.aa.bundlerUrl),
  });

  // 1. Make sure we have the attestation. If the receiveMessage call was
  //    already made by someone (incl. the user themselves), we can skip
  //    the IRIS poll entirely.
  let cctpMessage: Hex | undefined = params.attestation?.message;
  let cctpAttestation: Hex | undefined = params.attestation?.attestation;
  let receiveUserOpHash: Hex | undefined;

  // Only Arbitrum is a supported settlement chain today.
  const usdcAddress = USDC_ADDRESSES.Arbitrum;

  if (!cctpMessage || !cctpAttestation) {
    onProgress?.({ phase: "attestation" });
    const fetched = await fetchAttestation({
      sourceChain,
      txHash: burnTxHash,
      irisApiUrl,
      signal,
    });
    cctpMessage = fetched.message as Hex;
    cctpAttestation = fetched.attestation as Hex;
  }

  // 2. Decide whether to include receiveMessage. Use `usedNonces` — the
  //    authoritative signal (matches the logic in submit.ts).
  const nonce = extractNonce(cctpMessage);
  const usedNonce = (await publicClient.readContract({
    address: MESSAGE_TRANSMITTER_V2 as Address,
    abi: USED_NONCES_ABI,
    functionName: "usedNonces",
    args: [nonce],
  })) as bigint;
  const skipReceiveMessage = usedNonce !== 0n;

  // 3. Submit receiveMessage UserOp if the burn hasn't been claimed yet.
  if (!skipReceiveMessage) {
    const receiveData = encodeFunctionData({
      abi: MESSAGE_TRANSMITTER_ABI,
      functionName: "receiveMessage",
      args: [cctpMessage, cctpAttestation],
    });
    receiveUserOpHash = await aaClient.sendUserOperation({
      calls: [
        {
          to: MESSAGE_TRANSMITTER_V2 as Address,
          data: receiveData,
          value: 0n,
        },
      ],
    });
    onProgress?.({ phase: "receiving", userOpHash: receiveUserOpHash });
    await aaClient.waitForUserOperationReceipt({ hash: receiveUserOpHash });
  }

  // 4. Read the smart account's USDC balance — this is the recoverable amount.
  const balance = (await publicClient.readContract({
    address: usdcAddress as Address,
    abi: erc20Abi,
    functionName: "balanceOf",
    args: [accountAddress],
  })) as bigint;

  if (balance === 0n) {
    throw new Error(
      `No USDC at smart account ${accountAddress} after receiveMessage. Nothing to recover.`,
    );
  }

  // 5. Sweep the balance to `recipient`.
  const transferData = encodeFunctionData({
    abi: erc20Abi,
    functionName: "transfer",
    args: [recipient, balance],
  });
  const sweepUserOpHash = await aaClient.sendUserOperation({
    calls: [
      {
        to: usdcAddress as Address,
        data: transferData,
        value: 0n,
      },
    ],
  });
  onProgress?.({
    phase: "sweeping",
    userOpHash: sweepUserOpHash,
    amount: balance,
  });

  const sweepReceipt = await aaClient.waitForUserOperationReceipt({
    hash: sweepUserOpHash,
  });
  const sweepTxHash = sweepReceipt.receipt?.transactionHash as Hex | undefined;

  onProgress?.({
    phase: "done",
    receiveUserOpHash,
    sweepUserOpHash,
    sweepTxHash,
    recoveredAmount: balance,
  });

  return {
    receiveUserOpHash,
    sweepUserOpHash,
    sweepTxHash,
    smartAccountAddress: accountAddress as Address,
    recoveredAmount: balance,
  };
}
