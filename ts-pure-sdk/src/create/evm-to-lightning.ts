/**
 * EVM to Lightning swap creation.
 *
 * Supports swapping tokens from any EVM chain to pay a Lightning invoice.
 */

import type { EvmToLightningSwapResponse } from "../api/client.js";
import { bytesToHex } from "../signer/index.js";
import type {
  CreateSwapContext,
  EvmToLightningSwapGenericOptions,
  EvmToLightningSwapGenericResult,
} from "./types.js";

/**
 * Creates a new EVM to Lightning swap using the chain-agnostic generic endpoint.
 *
 * This allows users to swap any ERC-20 token from any supported EVM chain
 * to pay a Lightning invoice.
 *
 * @param options - The swap options.
 * @param ctx - The context containing API client and helper functions.
 * @returns The swap response and parameters for storage.
 * @throws Error if the swap creation fails.
 *
 * @example
 * ```ts
 * const result = await createEvmToLightningSwapGeneric(
 *   {
 *     lightningInvoice: "lnbc...",
 *     evmChainId: 137,               // Polygon
 *     tokenAddress: "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174", // USDC
 *     amountIn: 10000000,            // 10 USDC (6 decimals)
 *     userAddress: "0x1234...",
 *   },
 *   { apiClient, deriveSwapParams, storeSwap }
 * );
 * console.log("HTLC contract:", result.response.htlc_erc20_address);
 * ```
 */
export async function createEvmToLightningSwapGeneric(
  options: EvmToLightningSwapGenericOptions,
  ctx: CreateSwapContext,
): Promise<EvmToLightningSwapGenericResult> {
  const swapParams = await ctx.deriveSwapParams();
  // Note: For EVM-to-Lightning, hash_lock is derived from the Lightning invoice's payment_hash
  // by the server, so we don't send it here.
  const userId = bytesToHex(swapParams.userId);

  const body = {
    user_id: userId,
    lightning_invoice: options.lightningInvoice,
    evm_chain_id: options.evmChainId,
    token_address: options.tokenAddress,
    user_address: options.userAddress,
    referral_code: options.referralCode,
  };

  // Use fetch directly since the generated types don't have this endpoint yet
  const response = await fetch(`${ctx.baseUrl}/swap/evm/lightning`, {
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

  const data = (await response.json()) as EvmToLightningSwapResponse;

  // Store the swap if storage is configured
  await ctx.storeSwap(data.id, swapParams, {
    ...data,
    direction: "evm_to_lightning",
  });

  return { response: data, swapParams };
}
