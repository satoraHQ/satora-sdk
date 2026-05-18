/**
 * EVM to Bitcoin (on-chain) swap creation.
 *
 * Supports swapping tokens from Polygon, Arbitrum, or Ethereum to BTC on-chain
 * via Taproot HTLC.
 */

import { bytesToHex } from "../signer/index.js";
import { retryOnHashCollision } from "./retry.js";
import type {
  CreateSwapContext,
  EvmToBitcoinSwapOptions,
  EvmToBitcoinSwapResult,
} from "./types.js";

/**
 * Creates a new EVM-to-Bitcoin swap via the generic endpoint.
 *
 * Uses the chain-agnostic `/swap/evm/bitcoin` endpoint which supports any
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
 * const result = await createEvmToBitcoinSwap(
 *   {
 *     tokenAddress: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359", // USDC on Polygon
 *     evmChainId: 137,
 *     userAddress: "0x1234...",
 *     sourceAmount: 100000000n, // 100 USDC (6 decimals)
 *   },
 *   { apiClient, deriveSwapParams, storeSwap }
 * );
 * console.log("EVM HTLC:", result.response.evm_htlc_address);
 * console.log("BTC HTLC:", result.response.btc_htlc_address);
 * ```
 */
export async function createEvmToBitcoinSwap(
  options: EvmToBitcoinSwapOptions,
  ctx: CreateSwapContext,
): Promise<EvmToBitcoinSwapResult> {
  return retryOnHashCollision(ctx, async () => {
    const swapParams = await ctx.deriveSwapParams();
    const hashLock = `0x${bytesToHex(swapParams.preimageHash)}`;
    // For BTC on-chain claim, the user needs their public key (compressed, 33 bytes)
    const claimPk = bytesToHex(swapParams.publicKey);
    const userId = bytesToHex(swapParams.userId);

    const userAddress = options.gasless ? ctx.evmAddress : options.userAddress;

    const { data, error } = await ctx.apiClient.POST("/swap/evm/bitcoin", {
      body: {
        hash_lock: hashLock,
        claim_pk: claimPk,
        user_id: userId,
        token_address: options.tokenAddress,
        evm_chain_id: options.evmChainId,
        user_address: userAddress,
        target_address: options.targetAddress,
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
      direction: "evm_to_bitcoin",
    });

    return { response: data, swapParams };
  });
}
