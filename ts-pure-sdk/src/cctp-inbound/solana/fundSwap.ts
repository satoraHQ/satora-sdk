/**
 * `cctpFundSwapFromSolana` — CCTP-inbound settlement for USDC burned on
 * Solana.
 *
 * Same shape as {@link cctpFundSwap} for EVM sources; only the burn differs:
 *
 *   1. `deposit_for_burn` on Solana, signed and sent by the user's wallet,
 *      with `mintRecipient` / `destinationCaller` = the owner's Kernel
 *      account on Arbitrum.
 *   2. IRIS attestation for source domain 5, keyed by the transaction
 *      signature.
 *   3. The paymaster-sponsored settlement UserOp on Arbitrum, owned by
 *      `ownerSigner` (the SDK's key: a Solana wallet has no EVM account).
 *
 * Loaded through a dynamic `import()` from the `Client` so EVM-only
 * consumers never pull in the ed25519 code it needs.
 */

import { base64 } from "@scure/base";
import type { Chain, Hex } from "viem";
import { arbitrum } from "viem/chains";
import { fetchAttestation } from "../../cctp/attestation.js";
import {
  CCTP_DOMAINS,
  FINALITY_FAST,
  IRIS_API_MAINNET,
} from "../../cctp/constants.js";
import type { EvmSigner } from "../../evm/wallet.js";
import type { CctpFundSwapContext } from "../fundSwap.js";
import { createSwapSmartAccountClient } from "../smartAccount.js";
import { submitCctpInboundUserOp } from "../submit.js";
import {
  buildSolanaBurnTransaction,
  depositForBurnPdas,
  deriveAssociatedTokenAccount,
} from "./burn.js";
import {
  getBlockHeight,
  getLatestBlockhash,
  getSignatureStatus,
  getTokenAccountAmount,
  simulateTransaction,
  suggestPriorityFee,
} from "./rpc.js";
import type { SolanaSigner } from "./types.js";

/** Progress events of a Solana-sourced CCTP-inbound funding. */
export type SolanaCctpProgressStep =
  | { phase: "signing" }
  | { phase: "burning"; burnSignature: string }
  | { phase: "attestation" }
  | { phase: "submitting" }
  | { phase: "settled"; userOpHash: Hex; transactionHash?: Hex };

export interface CctpFundSwapFromSolanaParams {
  swapId: string;
  /** The user's Solana wallet: owns the USDC, signs and sends the burn. */
  signer: SolanaSigner;
  /** Kernel-account owner on the settlement chain; must be the swap's depositor. */
  ownerSigner: EvmSigner;
  /** Solana JSON-RPC URL used to build, simulate and confirm the burn. */
  rpcUrl: string;
  /** A burn this swap already sent: skips the burn, resumes at confirmation. */
  burnSignature?: string;
  /**
   * Called with the burn's signature as soon as the wallet sent it, and with
   * `undefined` if it then failed or expired (nothing was burned, the next
   * attempt must burn again). Persist it: it is what resuming or recovering
   * the settlement needs.
   */
  onBurn?: (burnSignature: string | undefined) => void | Promise<void>;
  /** USDC amount in smallest units (6 decimals). */
  amount: bigint;
  /** Max fast-transfer fee in USDC units. */
  maxFee: bigint;
  /** Priority fee in micro-lamports per CU. Defaults to recent network fees. */
  priorityFeeMicroLamports?: bigint;
  /** Settlement chain. Defaults to Arbitrum mainnet. */
  settlementChain?: Chain;
  /** IRIS API base URL. Defaults to mainnet. */
  irisApiUrl?: string;
  onProgress?: (step: SolanaCctpProgressStep) => void;
  signal?: AbortSignal;
  /** Poll interval for the burn's confirmation. Tests shorten it. */
  pollIntervalMs?: number;
}

export interface CctpFundSwapFromSolanaResult {
  /** Solana burn transaction signature (base58). */
  burnSignature: string;
  userOpHash: Hex;
  transactionHash?: Hex;
  smartAccountAddress: Hex;
}

/**
 * How long a resumed burn may stay unknown to the RPC before it is taken as
 * dropped. Longer than a blockhash lives (150 blocks, ~60-90 s), so a burn
 * sent just before the page reloaded has either landed or expired by then.
 */
const RESUME_LOOKUP_MS = 120_000;

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    signal?.throwIfAborted();
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener(
      "abort",
      () => {
        clearTimeout(timer);
        reject(new DOMException("Aborted", "AbortError"));
      },
      { once: true },
    );
  });
}

/**
 * Wait until the burn is confirmed. A burn that failed on-chain, or never
 * landed before its blockhash expired (or, when resuming without one, within
 * {@link RESUME_LOOKUP_MS}), clears the recorded signature and throws:
 * nothing left Solana and the next attempt must burn again.
 */
async function confirmSolanaBurn(
  rpcUrl: string,
  signature: string,
  expiry: { lastValidBlockHeight: number } | { deadline: number },
  onBurn: CctpFundSwapFromSolanaParams["onBurn"],
  pollIntervalMs: number,
  signal?: AbortSignal,
): Promise<void> {
  for (;;) {
    const status = await getSignatureStatus(rpcUrl, signature);
    if (status?.err != null) {
      await onBurn?.(undefined);
      throw new Error(
        `CCTP burn ${signature} failed on Solana: ${JSON.stringify(status.err)}`,
      );
    }
    if (
      status?.confirmationStatus === "confirmed" ||
      status?.confirmationStatus === "finalized"
    ) {
      return;
    }
    if (!status) {
      const expired =
        "deadline" in expiry
          ? Date.now() > expiry.deadline
          : (await getBlockHeight(rpcUrl)) > expiry.lastValidBlockHeight;
      if (expired) {
        await onBurn?.(undefined);
        throw new Error(
          `CCTP burn ${signature} never landed on Solana. No USDC left the wallet; try again.`,
        );
      }
    }
    await sleep(pollIntervalMs, signal);
  }
}

export async function cctpFundSwapFromSolana(
  context: CctpFundSwapContext,
  params: CctpFundSwapFromSolanaParams,
): Promise<CctpFundSwapFromSolanaResult> {
  const {
    swapId,
    signer,
    ownerSigner,
    rpcUrl,
    burnSignature: resumeSignature,
    onBurn,
    amount,
    maxFee,
    settlementChain = arbitrum,
    irisApiUrl = IRIS_API_MAINNET,
    onProgress,
    signal,
    pollIntervalMs = 2_000,
  } = params;

  const { accountAddress: smartAccountAddress } =
    await createSwapSmartAccountClient({
      signer: ownerSigner,
      aa: context.aa,
      chain: settlementChain,
    });

  // 1. The Solana burn, unless a previous attempt already sent one.
  let burnSignature: string;
  if (resumeSignature) {
    burnSignature = resumeSignature;
    onProgress?.({ phase: "burning", burnSignature });
    await confirmSolanaBurn(
      rpcUrl,
      burnSignature,
      { deadline: Date.now() + RESUME_LOOKUP_MS },
      onBurn,
      pollIntervalMs,
      signal,
    );
  } else {
    const destinationDomain = CCTP_DOMAINS.Arbitrum;
    const pdas = depositForBurnPdas(signer.publicKey, destinationDomain);
    const [balance, { blockhash, lastValidBlockHeight }, priorityFee] =
      await Promise.all([
        getTokenAccountAmount(
          rpcUrl,
          deriveAssociatedTokenAccount(signer.publicKey),
        ),
        getLatestBlockhash(rpcUrl),
        params.priorityFeeMicroLamports ??
          suggestPriorityFee(rpcUrl, [
            pdas.messageTransmitter,
            pdas.localToken,
          ]),
      ]);
    if (balance < amount) {
      throw new Error(
        `Not enough USDC on Solana: the swap needs ${amount} (6 decimals), the wallet's USDC account holds ${balance}.`,
      );
    }
    const burn = buildSolanaBurnTransaction({
      owner: signer.publicKey,
      amount,
      destinationDomain,
      mintRecipient: smartAccountAddress,
      destinationCaller: smartAccountAddress,
      maxFee,
      // Solana supports Fast Transfer.
      minFinalityThreshold: FINALITY_FAST,
      recentBlockhash: blockhash,
      priorityFeeMicroLamports: priorityFee,
    });

    // Catch a missing balance or token account before the wallet prompts.
    const failure = await simulateTransaction(
      rpcUrl,
      base64.encode(burn.transaction),
    );
    if (failure) {
      throw new Error(
        `The Solana burn would fail: ${JSON.stringify(failure.err)}${
          failure.logs.length ? `\n${failure.logs.join("\n")}` : ""
        }`,
      );
    }

    onProgress?.({ phase: "signing" });
    burnSignature = await signer.signAndSendTransaction(burn.transaction);
    await onBurn?.(burnSignature);
    onProgress?.({ phase: "burning", burnSignature });
    await confirmSolanaBurn(
      rpcUrl,
      burnSignature,
      { lastValidBlockHeight },
      onBurn,
      pollIntervalMs,
      signal,
    );
  }

  // 2. IRIS indexes Solana burns by source domain 5 + transaction signature.
  onProgress?.({ phase: "attestation" });
  const { message, attestation } = await fetchAttestation({
    sourceChain: "Solana",
    txHash: burnSignature,
    irisApiUrl,
    signal,
  });

  // 3. The settlement UserOp on Arbitrum.
  onProgress?.({ phase: "submitting" });
  const submit = await submitCctpInboundUserOp(context, {
    swapId,
    signer: ownerSigner,
    cctpMessage: message as Hex,
    cctpAttestation: attestation as Hex,
    chain: settlementChain,
  });
  onProgress?.({
    phase: "settled",
    userOpHash: submit.userOpHash,
    transactionHash: submit.transactionHash,
  });

  return {
    burnSignature,
    userOpHash: submit.userOpHash,
    transactionHash: submit.transactionHash,
    smartAccountAddress: smartAccountAddress as Hex,
  };
}
