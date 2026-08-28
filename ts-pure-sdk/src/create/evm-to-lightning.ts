/**
 * EVM to Lightning swap creation.
 *
 * The user locks an ERC-20 token on an EVM chain in an HTLCErc20 (via the
 * coordinator) whose hash lock is the payment hash of the Lightning
 * invoice they want paid; the server pays the invoice and claims the HTLC
 * with the revealed preimage.
 */

import { bytesToHex } from "../signer/index.js";
import type {
  CreateSwapContext,
  EvmToLightningSwapOptions,
  EvmToLightningSwapResult,
} from "./types.js";

/**
 * Creates a new EVM to Lightning swap.
 *
 * Flow:
 * 1. User funds the HTLCErc20 (`fundSwap` / `fundSwapGasless`) with `source_amount` token units
 * 2. Once the funding is final the server pays the Lightning invoice; the settled payment reveals
 *    the preimage
 * 3. Server claims the HTLC with the preimage
 *
 * The destination is **one of** `lightningInvoice` (its amount pins the
 * payout), or `lightningAddress`/`lnurl` with one of `sourceAmount`
 * (send-max: fees deducted from the payout) or `targetAmountSats` (exact
 * payout: fees added on top).
 *
 * Like Arkade → Lightning, the swap's hash lock is the invoice's payment
 * hash — the derived swap key is the EVM depositor for gasless funding and
 * the refund signer, so a duplicate-hash 409 is not retried with a new
 * key: it means this exact invoice already has a swap.
 *
 * @param options - The swap options.
 * @param ctx - The context containing API client and helper functions.
 * @returns The swap response and parameters for storage.
 * @throws Error if the swap creation fails.
 */
export async function createEvmToLightningSwap(
  options: EvmToLightningSwapOptions,
  ctx: CreateSwapContext,
): Promise<EvmToLightningSwapResult> {
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
    (options.sourceAmount === undefined) ===
      (options.targetAmountSats === undefined)
  ) {
    throw new Error("Provide exactly one of sourceAmount or targetAmountSats");
  }
  if (!options.gasless && !options.userAddress) {
    throw new Error("userAddress is required unless the swap is gasless");
  }

  const swapParams = await ctx.deriveSwapParams();
  const userId = bytesToHex(swapParams.userId);
  const userAddress = options.gasless ? ctx.evmAddress : options.userAddress;

  const { data, error } = await ctx.apiClient.POST("/swap/evm/lightning", {
    body: {
      lightning_invoice: options.lightningInvoice,
      lightning_address: options.lightningAddress,
      lnurl: options.lnurl,
      evm_chain_id: options.evmChainId,
      token_address: options.tokenAddress,
      amount_in:
        options.sourceAmount === undefined
          ? undefined
          : options.sourceAmount.toString(),
      target_amount_sats: options.targetAmountSats,
      user_address: userAddress ?? "",
      user_id: userId,
      referral_code: options.referralCode,
      extra_fees: options.extraFees,
      gasless: options.gasless ?? false,
      bridge_source_chain: options.inboundBridgeParams?.sourceChain,
      bridge_source_token_address:
        options.inboundBridgeParams?.sourceTokenAddress,
    },
  });
  if (error) {
    throw new Error(`Failed to create swap: ${JSON.stringify(error)}`);
  }
  if (!data) {
    throw new Error("No swap data returned");
  }

  // Store the swap if storage is configured — the derived key is the
  // gasless depositor and what signs a collaborative refund of the HTLC.
  await ctx.storeSwap(data.id, swapParams, {
    ...data,
    direction: "evm_to_lightning",
  });

  return { response: data, swapParams };
}
