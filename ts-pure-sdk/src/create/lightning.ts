/**
 * Lightning to EVM swap creation.
 */

import { bytesToHex } from "../signer/index.js";
import { retryOnHashCollision } from "./retry.js";
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
 * If the server rejects the hash lock (duplicate or collision), the
 * function automatically retries with a new key index.
 *
 * @param options - The swap options.
 * @param ctx - The context containing API client and helper functions.
 * @returns The swap response and parameters for storage.
 * @throws Error if the swap creation fails after all retries.
 */
export async function createLightningToEvmSwapGeneric(
  options: LightningToEvmSwapGenericOptions,
  ctx: CreateSwapContext,
): Promise<LightningToEvmSwapGenericResult> {
  return retryOnHashCollision(ctx, async () => {
    const swapParams = await ctx.deriveSwapParams();
    const hashLock = `0x${bytesToHex(swapParams.preimageHash)}`;
    const refundPk = bytesToHex(swapParams.publicKey);
    const userId = bytesToHex(swapParams.userId);

    const claimingAddress = ctx.evmAddress;

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
      extra_fees: options.extraFees,
      gasless: options.gasless ?? true,
      bridge_target_chain: options.bridgeParams?.targetChain,
      bridge_target_token_address: options.bridgeParams?.targetTokenAddress,
      bridge_recipient_setup: options.bridgeParams?.recipientSetup,
    };
    const { data, error } = await ctx.apiClient.POST("/swap/lightning/evm", {
      body,
    });

    if (error) {
      throw new Error(`Failed to create swap: ${JSON.stringify(error)}`);
    }
    if (!data) {
      throw new Error("No swap data returned");
    }

    // Store the swap if storage is configured
    await ctx.storeSwap(data.id, swapParams, {
      ...data,
      direction: "lightning_to_evm",
    });

    return { response: data, swapParams };
  });
}
