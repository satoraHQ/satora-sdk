/**
 * Lightning to EVM swap creation.
 */

import type { LightningToEvmSwapResponse } from "../api/client.js";
import { deriveEvmAddress } from "../evm/index.js";
import { bytesToHex } from "../signer/index.js";
import type {
  CreateSwapContext,
  LightningToEvmSwapGenericOptions,
  LightningToEvmSwapGenericResult,
} from "./types.js";

/**
 * Creates a new Lightning to EVM swap using the chain-agnostic generic endpoint.
 *
 * The claiming address is derived internally from the swap's secret key,
 * allowing the SDK to sign gasless claims without user interaction.
 *
 * @param options - The swap options.
 * @param ctx - The context containing API client and helper functions.
 * @returns The swap response and parameters for storage.
 * @throws Error if the swap creation fails.
 *
 * @example
 * ```ts
 * const result = await createLightningToEvmSwapGeneric(
 *   {
 *     targetAddress: "0x5678...",   // User's final destination
 *     evmChainId: 137,              // Polygon
 *     tokenAddress: "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174", // USDC on Polygon
 *     amountIn: 100000,             // 100k sats
 *   },
 *   { apiClient, deriveSwapParams, storeSwap }
 * );
 * console.log("Pay this invoice:", result.response.ln_invoice);
 * ```
 */
export async function createLightningToEvmSwapGeneric(
  options: LightningToEvmSwapGenericOptions,
  ctx: CreateSwapContext,
): Promise<LightningToEvmSwapGenericResult> {
  const swapParams = await ctx.deriveSwapParams();
  const hashLock = `0x${bytesToHex(swapParams.preimageHash)}`;
  const refundPk = bytesToHex(swapParams.publicKey);
  const userId = bytesToHex(swapParams.userId);

  // The claiming address is derived from the swap's secret key.
  // This allows the SDK to sign gasless claims internally.
  const claimingAddress = deriveEvmAddress(swapParams.secretKey);

  const body = {
    hash_lock: hashLock,
    refund_pk: refundPk,
    user_id: userId,
    claiming_address: claimingAddress,
    target_address: options.targetAddress,
    evm_chain_id: options.evmChainId,
    token_address: options.tokenAddress,
    amount_in: options.amountIn,
    amount_out: options.amountOut,
    referral_code: options.referralCode,
    gasless: options.gasless ?? true,
  };

  // Use fetch directly since the generated types don't have this endpoint yet
  const response = await fetch(`${ctx.baseUrl}/swap/lightning/evm`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  });

  if (!response.ok) {
    const error = await response.text();
    throw new Error(`Failed to create swap: ${error}`);
  }

  const data = (await response.json()) as LightningToEvmSwapResponse;

  // Store the swap if storage is configured
  await ctx.storeSwap(data.id, swapParams, {
    ...data,
    direction: "lightning_to_evm",
  });

  return { response: data, swapParams };
}
