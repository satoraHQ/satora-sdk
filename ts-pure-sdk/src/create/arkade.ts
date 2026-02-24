/**
 * Arkade to EVM swap creation.
 */

import { deriveEvmAddress } from "../evm/index.js";
import { bytesToHex } from "../signer/index.js";
import type {
  ArkadeToEvmSwapOptions,
  ArkadeToEvmSwapResult,
  CreateSwapContext,
} from "./types.js";

/**
 * Creates a new Arkade-to-EVM swap via the generic endpoint.
 *
 * Uses the chain-agnostic `/swap/arkade/evm` endpoint which supports any
 * ERC-20 token reachable through 1inch aggregation. The response includes
 * `evm_coordinator_address` and optional `dex_call_data` for the
 * redeem-and-swap flow.
 *
 * @param options - The swap options.
 * @param ctx - The context containing API client and helper functions.
 * @returns The swap response and parameters for storage.
 * @throws Error if the swap creation fails.
 *
 * @example
 * ```ts
 * const result = await createArkadeToEvmSwapGeneric(
 *   {
 *     targetAddress: "0x1234...",
 *     tokenAddress: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359", // USDC on Polygon
 *     evmChainId: 137,
 *     sourceAmount: 100000, // 100k sats
 *   },
 *   { apiClient, deriveSwapParams, storeSwap }
 * );
 * console.log("Fund:", result.response.btc_vhtlc_address);
 * console.log("Coordinator:", result.response.evm_coordinator_address);
 * ```
 */
export async function createArkadeToEvmSwapGeneric(
  options: ArkadeToEvmSwapOptions,
  ctx: CreateSwapContext,
): Promise<ArkadeToEvmSwapResult> {
  const swapParams = await ctx.deriveSwapParams();
  const hashLock = `0x${bytesToHex(swapParams.preimageHash)}`;
  const refundPk = bytesToHex(swapParams.publicKey);
  const userId = bytesToHex(swapParams.userId);

  // The claiming address is derived from the swap's secret key.
  // This allows the SDK to sign gasless claims internally.
  const claimingAddress = deriveEvmAddress(swapParams.secretKey);

  // Target address is where tokens are swept after the claim (user's final destination).
  // This is required and stored on the server for use during redemption.

  const { data, error } = await ctx.apiClient.POST("/swap/arkade/evm", {
    body: {
      hash_lock: hashLock,
      refund_pk: refundPk,
      user_id: userId,
      claiming_address: claimingAddress,
      target_address: options.targetAddress,
      token_address: options.tokenAddress,
      evm_chain_id: options.evmChainId,
      amount_in: options.sourceAmount
        ? Number(options.sourceAmount)
        : undefined,
      amount_out: options.targetAmount
        ? Number(options.targetAmount)
        : undefined,
      referral_code: options.referralCode,
      gasless: options.gasless ?? true,
    },
  });

  if (error) {
    throw new Error(`Failed to create swap: ${JSON.stringify(error)}`);
  }
  if (!data) {
    throw new Error("No swap data returned");
  }

  // Store the swap if storage is configured
  await ctx.storeSwap(data.id, swapParams, {
    ...data,
    direction: "arkade_to_evm",
  });

  return { response: data, swapParams };
}
