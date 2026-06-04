/**
 * `cctpFundSwap` — one-shot CCTP-inbound settlement.
 *
 * Composes the three primitives into a single flow most consumers
 * can call directly:
 *
 *   1. `approveAndBurn` on the source chain.
 *   2. `waitForAttestation` against IRIS until the burn is attested.
 *   3. `submitUserOp` on the settlement chain via the caller's Kernel
 *      smart account (paymaster-sponsored).
 *
 * The caller's single `EvmSigner` serves double duty: source-chain
 * signer for step 1 AND Kernel owner for step 3. The smart-account
 * address is derived once up-front and pinned as
 * `mintRecipient` / `destinationCaller` on the burn.
 */

import type { Chain, Hex } from "viem";
import { arbitrum } from "viem/chains";
import type { ApiClient } from "../api/client.js";
import { fetchAttestation } from "../cctp/attestation.js";
import { IRIS_API_MAINNET } from "../cctp/constants.js";
import type { EvmSigner } from "../evm/wallet.js";
import { approveAndBurn } from "./approveAndBurn.js";
import { cctpMetaForChainId, finalityForChainId } from "./chainMap.js";
import { createSwapSmartAccountClient } from "./smartAccount.js";
import { submitCctpInboundUserOp } from "./submit.js";
import type { AaConfig } from "./types.js";

/**
 * High-level progress events emitted during a CCTP-inbound swap.
 * Consumers wire a single `onProgress` callback to drive UI updates
 * without coupling to the primitive-level mechanics.
 */
export type CctpProgressStep =
  | { phase: "approving" }
  | { phase: "burning"; burnTxHash: Hex }
  | { phase: "attestation" }
  | { phase: "submitting" }
  | { phase: "settled"; userOpHash: Hex; transactionHash?: Hex };

export interface CctpFundSwapParams {
  /** Swap ID from `client.createSwap(...)`. */
  swapId: string;
  /**
   * SDK signer bound to the **source chain**. Its `address` is the
   * source-chain sender AND the Kernel-account owner on the settlement
   * chain. Requires `signer.signMessage` for the CCTP path.
   */
  signer: EvmSigner;
  /** USDC amount in smallest units (6 decimals). */
  amount: bigint;
  /**
   * Max fast-transfer fee in USDC units (from IRIS's
   * `/v2/burn/USDC/fees` endpoint). Typical: ~1/10000 of `amount`.
   */
  maxFee: bigint;
  /**
   * Settlement chain. Defaults to Arbitrum mainnet — the only
   * supported settlement chain today.
   */
  settlementChain?: Chain;
  /**
   * IRIS API base URL. Defaults to mainnet; override for testnet.
   */
  irisApiUrl?: string;
  /** Invoked as the flow advances through its phases. */
  onProgress?: (step: CctpProgressStep) => void;
  /** Abort signal for cancelling the attestation wait. */
  signal?: AbortSignal;
}

export interface CctpFundSwapResult {
  /** Source-chain burn tx. */
  burnTxHash: Hex;
  /** Source-chain approve tx (omitted if allowance was already sufficient). */
  approveTxHash?: Hex;
  /** Settlement-chain UserOp hash. */
  userOpHash: Hex;
  /** Settlement-chain bundled tx hash (populated once the UserOp is mined). */
  transactionHash?: Hex;
  /** Deterministic Kernel smart-account address owning the HTLC deposit. */
  smartAccountAddress: Hex;
}

/** Context passed by `CctpInboundClient` when delegating to this function. */
export interface CctpFundSwapContext {
  apiClient: ApiClient;
  aa: AaConfig;
}

/**
 * Execute the full CCTP-inbound swap flow end to end. Most consumers
 * call this directly — the primitives (`approveAndBurn`,
 * `waitForAttestation`, `submitUserOp`) are available separately for
 * wizard-style UX that needs per-step control.
 */
export async function cctpFundSwap(
  context: CctpFundSwapContext,
  params: CctpFundSwapParams,
): Promise<CctpFundSwapResult> {
  const {
    swapId,
    signer,
    amount,
    maxFee,
    settlementChain = arbitrum,
    irisApiUrl = IRIS_API_MAINNET,
    onProgress,
    signal,
  } = params;

  // Derive CCTP source metadata from the signer's chain id. No need
  // for the caller to spell out "Optimism" or the numeric domain —
  // the signer already knows which chain it's on.
  const source = cctpMetaForChainId(signer.chainId);
  const destination = cctpMetaForChainId(settlementChain.id);

  // Sources without Fast Transfer (e.g. XDC) must burn at Standard
  // finality — requesting fast finality there charges the wrong fee tier
  // and the attestation never clears at the fast threshold.
  const minFinalityThreshold = finalityForChainId(signer.chainId);

  // Derive the Kernel smart-account address up front. Stable across
  // all three phases — we mint to it on settlement and gate the
  // `receiveMessage` call on it.
  const { accountAddress: smartAccountAddress } =
    await createSwapSmartAccountClient({
      signer,
      aa: context.aa,
      chain: settlementChain,
    });

  // 1. Source-chain approve + burn.
  onProgress?.({ phase: "approving" });
  const { approveTxHash, burnTxHash } = await approveAndBurn({
    signer,
    amount,
    usdcAddress: source.usdc,
    destinationDomain: destination.domain,
    smartAccountAddress,
    maxFee,
    minFinalityThreshold,
  });
  onProgress?.({ phase: "burning", burnTxHash });
  const burnReceipt = await signer.waitForReceipt(burnTxHash);
  if (burnReceipt.status !== "success") {
    throw new Error(`CCTP burn tx reverted: ${burnTxHash}`);
  }

  // 2. Wait for the IRIS attestation to clear.
  onProgress?.({ phase: "attestation" });
  const { message, attestation } = await fetchAttestation({
    sourceChain: source.name,
    txHash: burnTxHash,
    irisApiUrl,
    signal,
  });

  // 3. Submit the settlement UserOp and wait for the on-chain receipt.
  onProgress?.({ phase: "submitting" });
  const submit = await submitCctpInboundUserOp(context, {
    swapId,
    signer,
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
    approveTxHash,
    burnTxHash,
    userOpHash: submit.userOpHash,
    transactionHash: submit.transactionHash,
    smartAccountAddress: smartAccountAddress as Hex,
  };
}
