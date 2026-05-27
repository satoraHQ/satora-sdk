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

/**
 * CCTPv2 message header: the `destinationCaller` is a bytes32 field at
 * offset 108..140. The address occupies the trailing 20 bytes (left-
 * padded with zeros). In hex-string coordinates that's chars [242, 282).
 *
 * `receiveMessage` reverts with `Invalid caller for message` when
 * `msg.sender` doesn't match this field, so we check it client-side
 * before burning a UserOp + paymaster gas on a guaranteed failure.
 */
function extractDestinationCaller(cctpMessage: Hex): Address {
  return `0x${cctpMessage.slice(242, 282)}` as Address;
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
    // Guard: only `destinationCaller` can call receiveMessage. If the
    // current owner key derives a different account, the on-chain
    // revert ("Invalid caller for message") is opaque — surface the
    // mismatch with an actionable message instead.
    const destinationCaller = extractDestinationCaller(cctpMessage);
    if (
      destinationCaller.toLowerCase() !==
      (accountAddress as string).toLowerCase()
    ) {
      throw new Error(
        `Recovery wallet mismatch: this CCTP burn was sent to ${destinationCaller}, ` +
          `but your current wallet's smart-account address is ${accountAddress}. ` +
          `You must recover with the same mnemonic that created the original swap.`,
      );
    }

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
