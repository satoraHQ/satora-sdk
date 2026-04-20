/**
 * EVM to Lightning swap creation.
 *
 * Supports swapping tokens from any EVM chain to pay a Lightning invoice.
 */

import { bytesToHex } from "../signer/index.js";
import { DuplicateInvoiceError, isDuplicateInvoiceError } from "./retry.js";
import type {
  CreateSwapContext,
  EvmToLightningSwapGenericOptions,
  EvmToLightningSwapGenericResult,
} from "./types.js";

/**
 * Creates a new EVM to Lightning swap using the chain-agnostic generic endpoint.
 *
 * This allows users to swap any ERC-20 token from any supported EVM chain
 * to pay a Lightning invoice.
 *
 * @param options - The swap options.
 * @param ctx - The context containing API client and helper functions.
 * @returns The swap response and parameters for storage.
 * @throws Error if the swap creation fails.
 *
 * @example
 * ```ts
 * const result = await createEvmToLightningSwapGeneric(
 *   {
 *     lightningInvoice: "lnbc...",
 *     evmChainId: 137,               // Polygon
 *     tokenAddress: "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174", // USDC
 *     amountIn: 10000000,            // 10 USDC (6 decimals)
 *     userAddress: "0x1234...",
 *   },
 *   { apiClient, deriveSwapParams, storeSwap }
 * );
 * console.log("HTLC contract:", result.response.htlc_erc20_address);
 * ```
 */
export async function createEvmToLightningSwapGeneric(
  options: EvmToLightningSwapGenericOptions,
  ctx: CreateSwapContext,
): Promise<EvmToLightningSwapGenericResult> {
  const swapParams = await ctx.deriveSwapParams();
  // Note: For EVM-to-Lightning, hash_lock is derived from the Lightning invoice's payment_hash
  // by the server, so we don't send it here.
  const userId = bytesToHex(swapParams.userId);

  const userAddress = options.gasless ? ctx.evmAddress : options.userAddress;

  let lightningInvoice = null;
  let lightningAddress = null;
  let lnurl = null;
  let amountSats = null;
  if (options.lightningInvoice) {
    lightningInvoice = options.lightningInvoice;
  } else if (options.lightningAddress) {
    lightningAddress = options.lightningAddress;
    amountSats = options.amountSats;
  } else if (options.lnurl) {
    lnurl = options.lnurl;
    amountSats = options.amountSats;
  }

  const { data, error } = await ctx.apiClient.POST("/swap/evm/lightning", {
    body: {
      amount_sats: amountSats,
      evm_chain_id: options.evmChainId,
      gasless: options.gasless ?? false,
      lightning_address: lightningAddress,
      lightning_invoice: lightningInvoice,
      lnurl: lnurl,
      referral_code: options.referralCode,
      token_address: options.tokenAddress,
      user_address: userAddress,
      user_id: userId,
      bridge_source_chain: options.inboundBridgeParams?.sourceChain,
      bridge_source_token_address:
        options.inboundBridgeParams?.sourceTokenAddress,
    },
  });

  if (error) {
    const message =
      typeof error === "string" ? error : JSON.stringify(error, null, 2);

    if (isDuplicateInvoiceError(message)) {
      throw new DuplicateInvoiceError(message);
    }

    throw new Error(`Failed to create swap: ${message}`);
  }
  if (!data) {
    throw new Error("No swap data returned");
  }

  // Store the swap if storage is configured
  await ctx.storeSwap(data.id, swapParams, {
    ...data,
    direction: "evm_to_lightning",
  });

  return { response: data, swapParams };
}
