/**
 * `CctpInboundClient` — the `client.cctpInbound` namespace on the main
 * `Client`. Holds the AA (bundler + paymaster) config and will expose
 * the step-by-step primitives for the CCTP-inbound flow:
 *
 *   - `approveAndBurn` (source-chain burn)
 *   - `waitForAttestation` (IRIS polling)
 *   - `submitUserOp` (settlement-chain UserOp via Kernel smart account)
 *   - `createSmartAccountClient` (low-level Kernel account factory)
 *
 * The one-shot `Client.fundSwap(...)` is implemented on top of these
 * primitives — consumers wanting custom progress UX drop down to the
 * primitives directly; simple integrations just call `fundSwap`.
 *
 * Scaffolding only in this commit; primitives land in follow-up steps.
 */

import type { Chain } from "viem";
import type { ApiClient } from "../api/client.js";
import {
  type AttestationResult,
  type FetchAttestationOptions,
  fetchAttestation,
} from "../cctp/attestation.js";
import type { EvmSigner } from "../evm/wallet.js";
import type { Logger, LogLevel } from "../logging.js";
import {
  type ApproveAndBurnParams,
  type ApproveAndBurnResult,
  approveAndBurn,
} from "./approveAndBurn.js";
import {
  type CctpFundSwapParams,
  type CctpFundSwapResult,
  cctpFundSwap,
} from "./fundSwap.js";
import {
  type CreateSwapSmartAccountClientParams,
  createSwapSmartAccountClient,
} from "./smartAccount.js";
import {
  type SubmitUserOpParams,
  type SubmitUserOpResult,
  submitCctpInboundUserOp,
} from "./submit.js";
import type { AaConfig } from "./types.js";
import {
  type BuildCctpInboundBatchParams,
  type BuiltBatch,
  buildCctpInboundBatch,
} from "./userOp.js";

export interface CctpInboundClientConfig {
  apiClient: ApiClient;
  aa: AaConfig;
  /** Optional logger sink. Silent by default. */
  logger?: Logger;
  /** Minimum log level to emit. Defaults to `silent`. */
  logLevel?: LogLevel;
}

export class CctpInboundClient {
  readonly #apiClient: ApiClient;
  readonly #aa: AaConfig;
  readonly #logger?: Logger;
  readonly #logLevel?: LogLevel;

  constructor(config: CctpInboundClientConfig) {
    this.#apiClient = config.apiClient;
    this.#aa = config.aa;
    this.#logger = config.logger;
    this.#logLevel = config.logLevel;
  }

  /** The AA config this client was built with. */
  get aa(): AaConfig {
    return this.#aa;
  }

  /** The underlying typed API client. */
  get api(): ApiClient {
    return this.#apiClient;
  }

  /**
   * Build a Kernel smart-account client owned by `owner`.
   *
   * Low-level primitive — most consumers will use `submitUserOp` or
   * the one-shot `client.fundSwap` instead of constructing the AA
   * client themselves.
   */
  createSmartAccountClient(args: { signer: EvmSigner; chain?: Chain }) {
    const params: CreateSwapSmartAccountClientParams = {
      signer: args.signer,
      aa: this.#aa,
      chain: args.chain,
    };
    return createSwapSmartAccountClient(params);
  }

  /**
   * Compose the 3-call UserOp batch (receiveMessage + USDC.approve +
   * executeAndCreateWithPermit2) that the smart account will execute
   * atomically. Produces the Permit2 signature via the caller-supplied
   * `signTypedData` — typically `kernelAccount.signTypedData`.
   *
   * Low-level primitive — most consumers call `submitUserOp` instead,
   * which composes batch + send + wait-for-receipt.
   */
  buildUserOpBatch(params: BuildCctpInboundBatchParams): Promise<BuiltBatch> {
    return buildCctpInboundBatch(params);
  }

  /**
   * End-to-end settlement UserOp: fetch calldata, build Kernel client,
   * compose the batch, submit via the bundler, and (by default) wait
   * for the on-chain receipt.
   *
   * This is the main high-level entry point for the CCTP-inbound
   * settlement step. Pass the IRIS message + attestation (from
   * `waitForAttestation`, landing in a follow-up step) and the
   * caller's signer.
   */
  submitUserOp(params: SubmitUserOpParams): Promise<SubmitUserOpResult> {
    return submitCctpInboundUserOp(
      { apiClient: this.#apiClient, aa: this.#aa },
      {
        ...params,
        logger: params.logger ?? this.#logger,
        logLevel: params.logLevel ?? this.#logLevel,
      },
    );
  }

  /**
   * Source-chain USDC approve (if needed) + `depositForBurn`. The
   * minted USDC on the destination chain is pinned to
   * `smartAccountAddress`, and `destinationCaller` is pinned to the
   * same address — only that account can settle the message.
   *
   * Does not wait for CCTP attestation; returns as soon as both txs
   * are broadcast. The caller pairs this with `waitForAttestation`
   * (landing in the next step) and `submitUserOp`.
   */
  approveAndBurn(params: ApproveAndBurnParams): Promise<ApproveAndBurnResult> {
    return approveAndBurn(params);
  }

  /**
   * Poll Circle's IRIS V2 API until the CCTP attestation for a burn
   * tx is `complete`, then return the `message` + `attestation` bytes
   * ready to feed into `submitUserOp`.
   *
   * Typical wait: ~15 seconds for fast transfers, a few minutes for
   * standard. Defaults to a 15-minute timeout; callers can pass
   * `signal` (AbortSignal) to cancel early.
   */
  waitForAttestation(
    options: FetchAttestationOptions,
  ): Promise<AttestationResult> {
    return fetchAttestation(options);
  }

  /**
   * One-shot CCTP-inbound swap: approve + burn on the source chain,
   * poll IRIS for the attestation, then submit the settlement UserOp
   * via the Kernel smart account. Paymaster sponsors the settlement
   * so the user needs no ETH on the destination chain.
   *
   * The `walletClient.account` signs both the source-chain txs and
   * the destination-chain UserOp (as Kernel owner), and its address
   * anchors the deterministic smart-account address.
   */
  fundSwap(params: CctpFundSwapParams): Promise<CctpFundSwapResult> {
    return cctpFundSwap({ apiClient: this.#apiClient, aa: this.#aa }, params);
  }
}
