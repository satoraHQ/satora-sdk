/**
 * Arkade to Lightning swap creation.
 *
 * The user sends Arkade VTXOs and a Lightning invoice gets paid
 * via a Boltz submarine swap.
 */

import { bytesToHex } from "../signer/index.js";
import { DuplicateInvoiceError, isDuplicateInvoiceError } from "./retry.js";
import type {
  ArkadeToLightningSwapOptions,
  ArkadeToLightningSwapResult,
  CreateSwapContext,
} from "./types.js";

/**
 * Creates a new Arkade to Lightning swap.
 *
 * Flow:
 * 1. User provides a Lightning invoice (or address) they want paid
 * 2. Server creates a Boltz submarine swap and returns an Arkade VHTLC address
 * 3. User funds the Arkade VHTLC
 * 4. Server claims the Arkade VHTLC and funds the Boltz VHTLC
 * 5. Boltz pays the Lightning invoice
 *
 * @param options - The swap options.
 * @param ctx - The context containing API client and helper functions.
 * @returns The swap response and parameters for storage.
 * @throws Error if the swap creation fails.
 *
 * @example
 * ```ts
 * const result = await createArkadeToLightningSwap(
 *   {
 *     lightningInvoice: "lnbc100u1p...",
 *   },
 *   { apiClient, deriveSwapParams, storeSwap }
 * );
 * console.log("Fund this address:", result.response.arkade_vhtlc_address);
 * console.log("Amount:", result.response.source_amount, "sats");
 * ```
 */
export async function createArkadeToLightningSwap(
  options: ArkadeToLightningSwapOptions,
  ctx: CreateSwapContext,
): Promise<ArkadeToLightningSwapResult> {
  const swapParams = await ctx.deriveSwapParams();
  const publicKey = bytesToHex(swapParams.publicKey);
  const userId = bytesToHex(swapParams.userId);

  const body: Record<string, unknown> = {
    refund_pk: publicKey,
    user_id: userId,
    referral_code: options.referralCode,
    extra_fees: options.extraFees,
  };

  if (options.lightningInvoice) {
    body.lightning_invoice = options.lightningInvoice;
  } else if (options.lightningAddress) {
    body.lightning_address = options.lightningAddress;
    body.amount_sats = options.amountSats;
  } else if (options.lnurl) {
    body.lnurl = options.lnurl;
    body.amount_sats = options.amountSats;
  }

  const { data, error } = await ctx.apiClient.POST("/swap/arkade/lightning", {
    body: body as never,
  });
  if (error) {
    const msg = JSON.stringify(error);
    if (isDuplicateInvoiceError(msg)) {
      throw new DuplicateInvoiceError(msg);
    }
    throw new Error(`Failed to create swap: ${msg}`);
  }
  if (!data) throw new Error("No swap data returned");

  // Store the swap if storage is configured
  await ctx.storeSwap(data.id, swapParams, {
    ...data,
    direction: "arkade_to_lightning",
  });

  return { response: data, swapParams };
}
