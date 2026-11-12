/**
 * Bitcoin (on-chain) to Arkade swap creation.
 *
 * The user sends on-chain BTC to a Taproot HTLC address and receives
 * Arkade VTXOs after the server funds the Arkade VHTLC.
 */

import { ripemd160 } from "@noble/hashes/legacy.js";
import { parseArkadeAddress } from "../arkade-address.js";
import { bytesToHex } from "../signer/index.js";
import {
  ARKADE_HTLC_SCRIPT_VERSION_STRICT,
  BTC_HTLC_SCRIPT_VERSION_STRICT,
} from "../strict-vhtlc.js";
import {
  assertAddressEchoed,
  assertBitcoinToArkadeHtlc,
} from "../swap-verification.js";
import { retryOnHashCollision } from "./retry.js";
import type {
  BitcoinToArkadeSwapOptions,
  BitcoinToArkadeSwapResult,
  CreateSwapContext,
} from "./types.js";

/**
 * Creates a new Bitcoin (on-chain) to Arkade swap.
 *
 * Flow:
 * 1. User sends BTC to the Taproot HTLC address returned
 * 2. After 1 confirmation, server creates Arkade VHTLC for user
 * 3. User claims Arkade VHTLC with secret, revealing it
 * 4. Server claims on-chain BTC with the revealed secret
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
 * const result = await createBitcoinToArkadeSwap(
 *   {
 *     satsReceive: 100000, // 100k sats to receive on Arkade
 *     targetAddress: "ark1q...", // Arkade address
 *   },
 *   { apiClient, deriveSwapParams, storeSwap }
 * );
 * console.log("Send BTC to:", result.response.btc_htlc_address);
 * console.log("Amount to send:", result.response.source_amount, "sats");
 * ```
 */
export async function createBitcoinToArkadeSwap(
  options: BitcoinToArkadeSwapOptions,
  ctx: CreateSwapContext,
): Promise<BitcoinToArkadeSwapResult> {
  parseArkadeAddress(options.targetAddress);

  return retryOnHashCollision(ctx, async () => {
    const swapParams = await ctx.deriveSwapParams();

    // Bitcoin-to-Arkade uses HASH160 (RIPEMD160(SHA256(preimage))) not SHA256
    const hash160 = ripemd160(swapParams.preimageHash);
    const hashLock = bytesToHex(hash160);

    // Both claim_pk (for Arkade VHTLC) and refund_pk (for on-chain HTLC)
    // use the same derived public key
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
      btc_htlc_script_version: BTC_HTLC_SCRIPT_VERSION_STRICT,
      arkade_htlc_script_version: ARKADE_HTLC_SCRIPT_VERSION_STRICT,
    };

    const { data, error } = await ctx.apiClient.POST("/swap/bitcoin/arkade", {
      body,
    });
    if (error)
      throw new Error(`Failed to create swap: ${JSON.stringify(error)}`);
    if (!data) throw new Error("No swap data returned");

    const response = data;
    // The user funds btc_htlc_address, so it must be an HTLC this client
    // can refund before the swap is stored or returned.
    assertBitcoinToArkadeHtlc(
      response,
      swapParams.preimageHash,
      swapParams.publicKey,
    );

    // The claim pays this address, so it must be the one this client sent.
    assertAddressEchoed(data, "target_arkade_address", options.targetAddress);

    // Store the swap if storage is configured
    await ctx.storeSwap(
      response.id,
      swapParams,
      { ...response, direction: "btc_to_arkade" },
      options.targetAddress,
    );

    return { response, swapParams };
  });
}
