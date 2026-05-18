/**
 * EVM to Arkade swap creation.
 *
 * Supports swapping tokens from Polygon, Arbitrum, or Ethereum to BTC on Arkade.
 */

import { bytesToHex } from "../signer/index.js";
import { retryOnHashCollision } from "./retry.js";
import type {
  CreateSwapContext,
  EvmToArkadeSwapGenericOptions,
  EvmToArkadeSwapGenericResult,
} from "./types.js";

/**
 * Creates a new EVM-to-Arkade swap via the generic endpoint.
 *
 * Uses the chain-agnostic `/swap/evm/arkade` endpoint which supports any
 * ERC-20 token reachable through 1inch aggregation.
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
 * const result = await createEvmToArkadeSwapGeneric(
 *   {
 *     targetAddress: "ark1q...",
 *     tokenAddress: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359", // USDC on Polygon
 *     evmChainId: 137,
 *     userAddress: "0x1234...",
 *     sourceAmount: 100000000, // 100 USDC (6 decimals)
 *   },
 *   { apiClient, deriveSwapParams, storeSwap }
 * );
 * console.log("HTLC:", result.response.evm_htlc_address);
 * ```
 */
export async function createEvmToArkadeSwapGeneric(
  options: EvmToArkadeSwapGenericOptions,
  ctx: CreateSwapContext,
): Promise<EvmToArkadeSwapGenericResult> {
  return retryOnHashCollision(ctx, async () => {
    const swapParams = await ctx.deriveSwapParams();
    const hashLock = `0x${bytesToHex(swapParams.preimageHash)}`;
    const receiverPk = bytesToHex(swapParams.publicKey);
    const userId = bytesToHex(swapParams.userId);

    const userAddress = options.gasless ? ctx.evmAddress : options.userAddress;

    const { data, error } = await ctx.apiClient.POST("/swap/evm/arkade", {
      body: {
        hash_lock: hashLock,
        receiver_pk: receiverPk,
        user_id: userId,
        target_address: options.targetAddress,
        token_address: options.tokenAddress,
        evm_chain_id: options.evmChainId,
        user_address: userAddress,
        amount_in: options.sourceAmount
          ? Number(options.sourceAmount)
          : undefined,
        amount_out: options.targetAmount
          ? Number(options.targetAmount)
          : undefined,
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

    // Store the swap if storage is configured
    await ctx.storeSwap(data.id, swapParams, {
      ...data,
      direction: "evm_to_arkade",
    });

    return { response: data, swapParams };
  });
}
