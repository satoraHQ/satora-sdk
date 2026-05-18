/**
 * Lightning to Arkade swap creation.
 *
 * The user pays a Lightning invoice and receives Arkade VTXOs
 * after Boltz funds the Arkade VHTLC.
 */

import { bytesToHex } from "../signer/index.js";
import { retryOnHashCollision } from "./retry.js";
import type {
  CreateSwapContext,
  LightningToArkadeSwapOptions,
  LightningToArkadeSwapResult,
} from "./types.js";

/**
 * Creates a new Lightning to Arkade swap.
 *
 * Flow:
 * 1. User pays the Lightning invoice returned
 * 2. Boltz receives BTC via Lightning and funds the Arkade VHTLC
 * 3. User claims Arkade VHTLC with secret, revealing it
 * 4. Server claims Boltz VHTLC with the revealed secret
 *
 * If the server rejects the hash lock (duplicate or collision), the
 * function automatically retries with a new key index.
 *
 * @param options - The swap options.
 * @param ctx - The context containing API client and helper functions.
 * @returns The swap response and parameters for storage.
 * @throws Error if the swap creation fails after all retries.
 *
 * @example
 * ```ts
 * const result = await createLightningToArkadeSwap(
 *   {
 *     satsReceive: 100000, // 100k sats to receive on Arkade
 *     targetAddress: "ark1q...", // Arkade address
 *   },
 *   { apiClient, deriveSwapParams, storeSwap }
 * );
 * console.log("Pay this invoice:", result.response.bolt11_invoice);
 * ```
 */
export async function createLightningToArkadeSwap(
  options: LightningToArkadeSwapOptions,
  ctx: CreateSwapContext,
): Promise<LightningToArkadeSwapResult> {
  return retryOnHashCollision(ctx, async () => {
    const swapParams = await ctx.deriveSwapParams();
    const hashLock = `0x${bytesToHex(swapParams.preimageHash)}`;
    const publicKey = bytesToHex(swapParams.publicKey);
    const userId = bytesToHex(swapParams.userId);

    const body = {
      hash_lock: hashLock,
      claim_pk: publicKey,
      refund_pk: publicKey,
      user_id: userId,
      sats_receive: options.satsReceive,
      target_arkade_address: options.targetAddress,
      referral_code: options.referralCode,
      extra_fees: options.extraFees,
    };

    const { data, error } = await ctx.apiClient.POST("/swap/lightning/arkade", {
      body,
    });
    if (error)
      throw new Error(`Failed to create swap: ${JSON.stringify(error)}`);
    if (!data) throw new Error("No swap data returned");

    // Store the swap if storage is configured
    await ctx.storeSwap(data.id, swapParams, {
      ...data,
      direction: "lightning_to_arkade",
    });

    return { response: data, swapParams };
  });
}
