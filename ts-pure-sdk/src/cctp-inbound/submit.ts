/**
 * `submitUserOp` — end-to-end UserOp submission for the CCTP-inbound
 * settlement step. Composes:
 *
 *   1. Fetch HTLC/DEX calldata from the backend.
 *   2. Build a Kernel smart-account client owned by the caller's signer.
 *   3. Check the smart-account USDC balance; skip `receiveMessage` if
 *      funds already landed (retry-safe).
 *   4. Compose the 3-call UserOp batch.
 *   5. Send the UserOp via the bundler; optionally wait for the
 *      on-chain tx hash.
 *
 * The authoritative high-level primitive for a settlement — most
 * consumers call this (or the even-higher-level `Client.fundSwap`).
 */

import type { Address, Chain, Hex } from "viem";
import { createPublicClient, erc20Abi, http } from "viem";
import { arbitrum } from "viem/chains";
import type { ApiClient } from "../api/client.js";
import type { EvmSigner } from "../evm/wallet.js";
import { simulateBatchCalls } from "./preflight.js";
import { createSwapSmartAccountClient } from "./smartAccount.js";
import type { AaConfig } from "./types.js";
import {
  buildCctpInboundBatch,
  type UseropCalldataResponse,
} from "./userOp.js";

export interface SubmitUserOpParams {
  /** Swap ID assigned by the backend at create time. */
  swapId: string;
  /**
   * Kernel smart-account owner as the SDK's `EvmSigner`. Requires
   * `signer.signMessage` (optional on `EvmSigner`) — Kernel's validator
   * signs the UserOp hash via it. Clear error thrown if missing.
   */
  signer: EvmSigner;
  /** Raw IRIS message bytes (hex, 0x-prefixed). */
  cctpMessage: Hex;
  /** Raw IRIS attestation bytes (hex, 0x-prefixed). */
  cctpAttestation: Hex;
  /**
   * Settlement chain. Defaults to Arbitrum mainnet — the only
   * supported chain today, parameterised for future extension.
   */
  chain?: Chain;
  /** Return immediately after submission without waiting for receipt. */
  noWait?: boolean;
  /**
   * When `true`, run each batched call individually via `eth_call`
   * before submission and log per-call status to the console. Useful
   * for iterating on ABI / calldata issues — bundler reverts are
   * aggregated and often opaque. Never throws; the bundler's real
   * simulation on `sendUserOperation` remains the authoritative check.
   */
  preflightSimulate?: boolean;
}

export interface SubmitUserOpResult {
  /** Bundler-assigned UserOperation hash. */
  userOpHash: Hex;
  /** Deterministic smart-account address (owned by `signer`). */
  smartAccountAddress: Address;
  /**
   * On-chain transaction hash for the bundle containing this UserOp.
   * Populated when `noWait !== true` — omitted when the caller opts
   * out of waiting.
   */
  transactionHash?: Hex;
}

/** Inputs needed from the `CctpInboundClient` to execute a submission. */
export interface SubmitUserOpContext {
  apiClient: ApiClient;
  aa: AaConfig;
}

/**
 * Execute the full CCTP-inbound settlement UserOp for a swap.
 *
 * This is intentionally a free function rather than a
 * `CctpInboundClient` method — the client exposes a thin wrapper that
 * injects its own state; tests and advanced consumers can call the
 * free function directly with a custom `SubmitUserOpContext`.
 */
export async function submitCctpInboundUserOp(
  context: SubmitUserOpContext,
  params: SubmitUserOpParams,
): Promise<SubmitUserOpResult> {
  const { apiClient, aa } = context;
  const {
    swapId,
    signer,
    cctpMessage,
    cctpAttestation,
    chain = arbitrum,
    noWait,
    preflightSimulate,
  } = params;

  // 1. Fetch the HTLC/DEX calldata the backend built for this swap.
  const { data, error } = await apiClient.GET(
    "/swap/{id}/swap-and-lock-calldata-userop",
    { params: { path: { id: swapId } } },
  );
  if (error || !data) {
    throw new Error(
      `Failed to fetch CCTP-inbound calldata for swap ${swapId}: ${JSON.stringify(
        error,
      )}`,
    );
  }
  const server = data as unknown as UseropCalldataResponse;

  // 2. Derive the Kernel client + the smart-account address.
  const {
    client: aaClient,
    account: smartAccount,
    accountAddress,
  } = await createSwapSmartAccountClient({ signer, aa, chain });

  // 3. Check if USDC already landed at the smart account — happens on
  //    retries or when a third-party relayer front-ran `receiveMessage`.
  //    Reuses the bundler URL as the node RPC (Alchemy serves both).
  const publicClient = createPublicClient({
    chain,
    transport: http(aa.bundlerUrl),
  });
  const usdcBalance = (await publicClient.readContract({
    address: server.source_token_address as `0x${string}`,
    abi: erc20Abi,
    functionName: "balanceOf",
    args: [accountAddress],
  })) as bigint;
  const skipReceiveMessage = usdcBalance >= BigInt(server.source_amount);

  // 4. Compose the 3-call batch (receiveMessage + approve + HTLC create).
  const { calls } = await buildCctpInboundBatch({
    server,
    smartAccountAddress: accountAddress,
    signTypedData: (args) => smartAccount.signTypedData(args),
    cctpMessage,
    cctpAttestation,
    chainId: chain.id,
    skipReceiveMessage,
  });

  // 5. Optional debug pre-flight: per-call `eth_call` with the smart
  //    account as `from`, logging per-call status. The bundler's
  //    full simulation at `sendUserOperation` is the authoritative
  //    check — this is purely for observability while iterating.
  if (preflightSimulate) {
    await simulateBatchCalls({
      calls,
      smartAccount: accountAddress,
      publicClient,
    });
  }

  // 6. Send the UserOp via the bundler. Paymaster sponsorship is wired
  //    into the Kernel client already, so no gas is owed by the caller.
  const userOpHash = await aaClient.sendUserOperation({ calls });

  if (noWait) {
    return { userOpHash, smartAccountAddress: accountAddress };
  }

  const receipt = await aaClient.waitForUserOperationReceipt({
    hash: userOpHash,
  });
  return {
    userOpHash,
    smartAccountAddress: accountAddress,
    transactionHash: receipt.receipt?.transactionHash,
  };
}
