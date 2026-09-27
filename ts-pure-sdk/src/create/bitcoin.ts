/**
 * Bitcoin (on-chain) to EVM swap creation via the generic endpoint.
 */

import { deriveEvmAddress } from "../evm/signing.js";
import { bytesToHex } from "../signer/index.js";
import { BTC_HTLC_SCRIPT_VERSION_STRICT } from "../strict-vhtlc.js";
import { retryOnHashCollision } from "./retry.js";
import type {
  BitcoinToEvmSwapOptions,
  BitcoinToEvmSwapResult,
  CreateSwapContext,
} from "./types.js";

/**
 * Creates a new Bitcoin (on-chain) to EVM swap.
 *
 * Uses the chain-agnostic `/swap/bitcoin/evm` endpoint which supports any
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
 * const result = await createBitcoinToEvmSwap(
 *   {
 *     targetAddress: "0x1234...",
 *     tokenAddress: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359", // USDC on Polygon
 *     evmChainId: 137,
 *     sourceAmount: 100000, // 100k sats
 *   },
 *   { apiClient, deriveSwapParams, storeSwap }
 * );
 * console.log("Send BTC to:", result.response.btc_htlc_address);
 * ```
 */
export async function createBitcoinToEvmSwap(
  options: BitcoinToEvmSwapOptions,
  ctx: CreateSwapContext,
): Promise<BitcoinToEvmSwapResult> {
  return retryOnHashCollision(ctx, async () => {
    const swapParams = await ctx.deriveSwapParams();
    const hashLock = `0x${bytesToHex(swapParams.preimageHash)}`;
    const refundPk = bytesToHex(swapParams.publicKey);
    const userId = bytesToHex(swapParams.userId);

    // The claiming address is this swap's own key, not the wallet-level
    // EVM address used for gasless deposits. A sponsored claim installs an
    // EIP-7702 delegation (Kernel) on the claiming address; keeping that
    // off the deposit address means Permit2 / EIP-2612 keep seeing a plain
    // EOA there. Recovery re-derives this key from the swap's index.
    const claimingAddress = deriveEvmAddress(swapParams.secretKey);

    const body = {
      hash_lock: hashLock,
      btc_htlc_script_version: BTC_HTLC_SCRIPT_VERSION_STRICT,
      refund_pk: refundPk,
      user_id: userId,
      claiming_address: claimingAddress,
      target_address: options.targetAddress,
      token_address: options.tokenAddress,
      evm_chain_id: options.evmChainId,
      amount_in: options.sourceAmount,
      amount_out: options.targetAmount?.toString(),
      referral_code: options.referralCode,
      extra_fees: options.extraFees,
      gasless: options.gasless ?? true,
      bridge_target_chain: options.bridgeParams?.targetChain,
      bridge_target_token_address: options.bridgeParams?.targetTokenAddress,
      bridge_recipient_setup: options.bridgeParams?.recipientSetup,
    };
    const { data, error } = await ctx.apiClient.POST("/swap/bitcoin/evm", {
      body,
    });

    if (error) {
      throw new Error(`Failed to create swap: ${JSON.stringify(error)}`);
    }
    if (!data) {
      throw new Error("No swap data returned");
    }

    // Store the swap if storage is configured
    await ctx.storeSwap(
      data.id,
      swapParams,
      {
        ...data,
        direction: "bitcoin_to_evm",
      },
      undefined,
      {
        recipient: options.bridgeParams?.recipient,
        recipientWallet: options.bridgeParams?.recipientWallet,
      },
    );

    return { response: data, swapParams };
  });
}
