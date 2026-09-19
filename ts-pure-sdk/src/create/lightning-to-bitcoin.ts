/**
 * Lightning to Bitcoin (on-chain) swap creation.
 *
 * The user pays a hold invoice locked to the SDK's hash; the server funds an
 * on-chain Taproot HTLC the user claims with the preimage, and the revealed
 * preimage settles the hold invoice.
 */
import { bytesToHex } from "../signer/index.js";
import { BTC_HTLC_SCRIPT_VERSION_STRICT } from "../strict-vhtlc.js";
import { retryOnHashCollision } from "./retry.js";
import type {
  CreateSwapContext,
  LightningToBitcoinSwapOptions,
  LightningToBitcoinSwapResult,
} from "./types.js";

/**
 * Creates a new Lightning to Bitcoin (on-chain) swap.
 *
 * Flow:
 * 1. User pays the returned hold invoice (`bolt11_invoice`)
 * 2. Server funds the on-chain HTLC (`btc_htlc_address`)
 * 3. User claims the HTLC with the preimage (`claim()`), which settles the
 *    hold invoice
 *
 * Provide exactly one of `sourceAmountSats` (invoice amount, fees deducted
 * from the payout) or `targetAmountSats` (exact on-chain payout, fees added
 * on top). `targetAddress` is the Bitcoin address the claim pays out to.
 *
 * @param options - The swap options.
 * @param ctx - The context containing API client and helper functions.
 * @returns The swap response and parameters for storage.
 * @throws Error if the swap creation fails.
 *
 * @example
 * ```ts
 * const result = await createLightningToBitcoinSwap(
 *   { targetAmountSats: 100000, targetAddress: "bc1q..." },
 *   { apiClient, deriveSwapParams, storeSwap }
 * );
 * console.log("Pay this invoice:", result.response.bolt11_invoice);
 * ```
 */
export async function createLightningToBitcoinSwap(
  options: LightningToBitcoinSwapOptions,
  ctx: CreateSwapContext,
): Promise<LightningToBitcoinSwapResult> {
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
    const claimPk = bytesToHex(swapParams.publicKey);
    const userId = bytesToHex(swapParams.userId);

    const body = {
      hash_lock: hashLock,
      claim_pk: claimPk,
      user_id: userId,
      source_amount_sats: options.sourceAmountSats,
      target_amount_sats: options.targetAmountSats,
      target_address: options.targetAddress,
      btc_htlc_script_version: BTC_HTLC_SCRIPT_VERSION_STRICT,
      referral_code: options.referralCode,
      extra_fees: options.extraFees,
      invoice_description: options.invoiceDescription,
    };

    const { data, error } = await ctx.apiClient.POST(
      "/swap/lightning/bitcoin",
      { body },
    );
    if (error) {
      throw new Error(`Failed to create swap: ${JSON.stringify(error)}`);
    }
    if (!data) {
      throw new Error("No swap data returned");
    }

    // The derived key claims the HTLC; the preimage settles the invoice.
    await ctx.storeSwap(data.id, swapParams, {
      ...data,
      direction: "lightning_to_bitcoin",
    });

    return { response: data, swapParams };
  });
}
