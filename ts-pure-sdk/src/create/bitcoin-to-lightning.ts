/**
 * Bitcoin (on-chain) to Lightning swap creation.
 *
 * The user locks BTC in an on-chain Taproot HTLC; the server pays the user's
 * Lightning invoice and claims the HTLC with the revealed preimage.
 */
import { bytesToHex } from "../signer/index.js";
import { BTC_HTLC_SCRIPT_VERSION_STRICT } from "../strict-vhtlc.js";
import type {
  BitcoinToLightningSwapOptions,
  BitcoinToLightningSwapResult,
  CreateSwapContext,
} from "./types.js";

/**
 * Creates a new Bitcoin (on-chain) to Lightning swap.
 *
 * Flow:
 * 1. User sends `source_amount` sats to the returned HTLC address (`btc_htlc_address`)
 * 2. After one confirmation the server pays the Lightning invoice; the settled
 *    payment reveals the preimage
 * 3. Server claims the HTLC with the preimage
 *
 * The destination is **one of** `lightningInvoice` (its amount pins the
 * payout), or `lightningAddress`/`lnurl` with one of `sourceAmountSats`
 * (send-max: fees deducted from the payout) or `targetAmountSats` (exact
 * payout: fees added on top).
 *
 * As for Arkade → Lightning, the swap's hash lock is the invoice's payment
 * hash; the derived swap key is only the HTLC's refund key, so a
 * duplicate-hash 409 is not retried with a new key: it means this exact
 * invoice already has a swap.
 *
 * @param options - The swap options.
 * @param ctx - The context containing API client and helper functions.
 * @returns The swap response and parameters for storage.
 * @throws Error if the swap creation fails.
 *
 * @example
 * ```ts
 * const result = await createBitcoinToLightningSwap(
 *   { lightningInvoice: "lnbc..." },
 *   { apiClient, deriveSwapParams, storeSwap }
 * );
 * console.log("Send BTC to:", result.response.btc_htlc_address);
 * console.log("Amount to send:", result.response.source_amount, "sats");
 * ```
 */
export async function createBitcoinToLightningSwap(
  options: BitcoinToLightningSwapOptions,
  ctx: CreateSwapContext,
): Promise<BitcoinToLightningSwapResult> {
  const destinations = [
    options.lightningInvoice,
    options.lightningAddress,
    options.lnurl,
  ].filter((d) => d !== undefined);
  if (destinations.length !== 1) {
    throw new Error(
      "Provide exactly one of lightningInvoice, lightningAddress or lnurl",
    );
  }
  if (
    options.lightningInvoice === undefined &&
    (options.sourceAmountSats === undefined) ===
      (options.targetAmountSats === undefined)
  ) {
    throw new Error(
      "Provide exactly one of sourceAmountSats or targetAmountSats",
    );
  }

  const swapParams = await ctx.deriveSwapParams();
  const refundPk = bytesToHex(swapParams.publicKey);
  const userId = bytesToHex(swapParams.userId);

  const body = {
    lightning_invoice: options.lightningInvoice,
    lightning_address: options.lightningAddress,
    lnurl: options.lnurl,
    source_amount_sats: options.sourceAmountSats,
    target_amount_sats: options.targetAmountSats,
    refund_pk: refundPk,
    user_id: userId,
    btc_htlc_script_version: BTC_HTLC_SCRIPT_VERSION_STRICT,
    referral_code: options.referralCode,
    extra_fees: options.extraFees,
  };

  const { data, error } = await ctx.apiClient.POST("/swap/bitcoin/lightning", {
    body,
  });
  if (error) {
    throw new Error(`Failed to create swap: ${JSON.stringify(error)}`);
  }
  if (!data) {
    throw new Error("No swap data returned");
  }

  // The derived key is what signs a refund of the HTLC after its locktime.
  await ctx.storeSwap(data.id, swapParams, {
    ...data,
    direction: "bitcoin_to_lightning",
  });

  return { response: data, swapParams };
}
