/**
 * Arkade to Lightning swap creation.
 *
 * The user locks Arkade VTXOs in a VHTLC; the server pays the user's
 * Lightning invoice and claims the VHTLC with the revealed preimage.
 */

import { invoicePaymentHash } from "../bolt11.js";
import { bytesToHex } from "../signer/index.js";
import { ARKADE_HTLC_SCRIPT_VERSION_STRICT } from "../strict-vhtlc.js";
import { checkedLightningPaymentHash } from "../swap-verification.js";
import type {
  ArkadeToLightningSwapOptions,
  ArkadeToLightningSwapResult,
  CreateSwapContext,
} from "./types.js";

/**
 * Creates a new Arkade to Lightning swap.
 *
 * Flow:
 * 1. User funds the returned Arkade VHTLC (`arkade_vhtlc_address`) with `source_amount` sats
 * 2. Server pays the Lightning invoice; the settled payment reveals the preimage
 * 3. Server claims the VHTLC with the preimage
 *
 * The destination is **one of** `lightningInvoice` (its amount pins the
 * payout), or `lightningAddress`/`lnurl` with one of `sourceAmountSats`
 * (send-max: fees deducted from the payout) or `targetAmountSats` (exact
 * payout: fees added on top).
 *
 * Unlike the other directions, the swap's hash lock is the invoice's
 * payment hash — the derived swap key is only used for the refund path
 * (`refund_pk`), so a duplicate-hash 409 is not retried with a new key:
 * it means this exact invoice already has a swap.
 *
 * Before the swap is stored or returned, `hash_lock` is checked to be the
 * payment hash of `client_lightning_invoice`, and that invoice to pay the
 * same hash as `lightningInvoice` when one was given. A lock on any other
 * hash would let the server claim the VHTLC without paying the invoice. With
 * `lightningAddress` or `lnurl` the server resolves the invoice, which must
 * then also ask for the payout the server quoted.
 *
 * @param options - The swap options.
 * @param ctx - The context containing API client and helper functions.
 * @returns The swap response and parameters for storage.
 * @throws {SwapVerificationError} if the server's hash lock or invoice does
 *   not match. Nothing is stored; do not fund the VHTLC.
 * @throws Error if `lightningInvoice` is not a BOLT11 invoice (before any
 *   request is made), or if the swap creation fails.
 *
 * @example
 * ```ts
 * const result = await createArkadeToLightningSwap(
 *   { lightningInvoice: "lnbc..." },
 *   { apiClient, deriveSwapParams, storeSwap }
 * );
 * console.log("Fund:", result.response.arkade_vhtlc_address);
 * console.log("Lock amount:", result.response.source_amount);
 * ```
 */
export async function createArkadeToLightningSwap(
  options: ArkadeToLightningSwapOptions,
  ctx: CreateSwapContext,
): Promise<ArkadeToLightningSwapResult> {
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
  // A malformed invoice is refused before a key is derived or the server asked.
  if (options.lightningInvoice !== undefined) {
    invoicePaymentHash(options.lightningInvoice);
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
    arkade_htlc_script_version: ARKADE_HTLC_SCRIPT_VERSION_STRICT,
    referral_code: options.referralCode,
    extra_fees: options.extraFees,
  };

  const { data, error } = await ctx.apiClient.POST("/swap/arkade/lightning", {
    body,
  });
  if (error) {
    throw new Error(`Failed to create swap: ${JSON.stringify(error)}`);
  }
  if (!data) {
    throw new Error("No swap data returned");
  }

  checkedLightningPaymentHash(options, data);

  // Store the swap if storage is configured — the derived key is what
  // signs a collaborative/unilateral refund of the VHTLC.
  await ctx.storeSwap(data.id, swapParams, {
    ...data,
    direction: "arkade_to_lightning",
  });

  return { response: data, swapParams };
}
