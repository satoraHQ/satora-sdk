/**
 * Lightning to Arkade swap creation.
 *
 * The user pays a Lightning hold invoice and receives Arkade VTXOs after
 * the server funds the Arkade VHTLC.
 */

import { parseArkadeAddress } from "../arkade-address.js";
import { bytesToHex } from "../signer/index.js";
import { ARKADE_HTLC_SCRIPT_VERSION_STRICT } from "../strict-vhtlc.js";
import { assertInvoicePays } from "../swap-verification.js";
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
 * 1. User pays the Lightning hold invoice returned
 * 2. The payment is held; the server funds the Arkade VHTLC
 * 3. User claims the Arkade VHTLC with the secret, revealing it
 * 4. Server settles the hold invoice with the revealed secret
 *
 * The returned invoice is decoded and its payment hash checked against the
 * hash of this swap's own secret before the swap is stored or returned. An
 * invoice on any other hash would let the server settle the payment without
 * ever funding the VHTLC.
 *
 * Provide **one of** `sourceAmountSats` (invoice amount, fees come out of
 * it) or `targetAmountSats` (exact sats to receive on Arkade).
 *
 * If the server rejects the hash lock (duplicate or collision), the
 * function automatically retries with a new key index.
 *
 * @param options - The swap options.
 * @param ctx - The context containing API client and helper functions.
 * @returns The swap response and parameters for storage.
 * @throws {SwapVerificationError} if the server's invoice does not pay this
 *   swap's hash lock. Nothing is stored; do not pay that invoice.
 * @throws Error if the swap creation fails after all retries.
 *
 * @example
 * ```ts
 * const result = await createLightningToArkadeSwap(
 *   {
 *     targetAmountSats: 100000, // 100k sats to receive on Arkade
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
  parseArkadeAddress(options.targetAddress);

  if (
    (options.sourceAmountSats === undefined) ===
    (options.targetAmountSats === undefined)
  ) {
    throw new Error(
      "Provide exactly one of sourceAmountSats or targetAmountSats",
    );
  }

  return retryOnHashCollision(ctx, async () => {
    const swapParams = await ctx.deriveSwapParams();
    const hashLock = `0x${bytesToHex(swapParams.preimageHash)}`;
    const publicKey = bytesToHex(swapParams.publicKey);
    const userId = bytesToHex(swapParams.userId);

    const body = {
      hash_lock: hashLock,
      claim_pk: publicKey,
      user_id: userId,
      source_amount_sats: options.sourceAmountSats,
      target_amount_sats: options.targetAmountSats,
      target_arkade_address: options.targetAddress,
      arkade_htlc_script_version: ARKADE_HTLC_SCRIPT_VERSION_STRICT,
      referral_code: options.referralCode,
      extra_fees: options.extraFees,
      // `undefined` is omitted from the JSON body so the server applies its
      // default; an explicit "" is sent to blank the invoice description.
      invoice_description: options.invoiceDescription,
    };

    const { data, error } = await ctx.apiClient.POST("/swap/lightning/arkade", {
      body,
    });
    if (error)
      throw new Error(`Failed to create swap: ${JSON.stringify(error)}`);
    if (!data) throw new Error("No swap data returned");

    assertInvoicePays(data.bolt11_invoice, hashLock);

    // Store the swap if storage is configured
    await ctx.storeSwap(data.id, swapParams, {
      ...data,
      direction: "lightning_to_arkade",
    });

    return { response: data, swapParams };
  });
}
