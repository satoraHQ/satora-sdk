/**
 * Redeem module for Lendaswap swaps.
 *
 * Provides redeem/claim logic for completing swaps:
 * - Server-side gasless claims for Arkade/Lightning-to-EVM swaps
 * - Manual claiming with call data for other EVM swaps
 * - Arkade VHTLC claiming for EVM-to-Arkade swaps
 * - Coordinator redeemAndExecute for Arkade-to-EVM swaps
 */

import type { ArkadeToEvmSwapResponse } from "../api/client.js";
import { buildEthereumClaimData } from "./ethereum.js";
import {
  type ArkadeClaimData,
  type ClaimResult,
  type CoordinatorClaimData,
  getChainFromTokenId,
  getClaimChainFromChainName,
  type RedeemContext,
} from "./types.js";

// Re-export Arkade claim
export {
  type ArkadeClaimParams,
  type ArkadeClaimResult,
  buildArkadeClaim,
  continueArkadeClaim,
} from "./arkade.js";
// Re-export utilities from ethereum module
export { encodeClaimSwapCallData, uuidToBytes32 } from "./ethereum.js";
// Re-export gasless claim
export { claimViaGasless, type GaslessClaimParams } from "./gasless.js";
// Re-export types
export type {
  ArkadeClaimData,
  ClaimChain,
  ClaimGaslessResult,
  ClaimResult,
  CoordinatorClaimData,
  EthereumClaimData,
  RedeemContext,
} from "./types.js";
export { getChainFromTokenId } from "./types.js";
export { claimViaUserOp, type UserOpClaimParams } from "./userop-claim.js";

/**
 * Claims a swap by revealing the preimage.
 *
 * The claim method depends on the swap direction and target chain:
 * - **Arkade/Lightning-to-EVM**: Server-side gasless claim via coordinator
 * - **Other EVM swaps**: Returns call data for manual claiming
 * - **Arkade**: Returns data needed for `buildArkadeClaim()`
 *
 * @param id - The UUID of the swap.
 * @param secret - The preimage/secret (32-byte hex string, with or without 0x prefix).
 * @param ctx - The context containing the API client and getSwap function.
 * @param destination - (Optional) EVM address for receiving tokens. Required for Arkade-to-EVM
 *                      swaps to fetch fresh DEX calldata.
 * @returns A ClaimResult with the outcome.
 *
 * @example
 * ```ts
 * const result = await claim(swapId, storedSwap.preimage, ctx, "0x1234...");
 * if (result.success) {
 *   if (result.coordinatorClaimData) {
 *     // Arkade/Lightning-to-EVM: gasless claim via server
 *     console.log("TX Hash:", result.txHash);
 *   } else if (result.chain === "arkade") {
 *     // Arkade claim needs user's keys
 *     await buildArkadeClaim({ ...result.arkadeClaimData, ... });
 *   } else {
 *     // Manual EVM claim
 *     console.log("Call data:", result.ethereumClaimData?.callData);
 *   }
 * }
 * ```
 */
export async function claim(
  id: string,
  secret: string,
  ctx: RedeemContext,
  destination?: string,
): Promise<ClaimResult> {
  // Get the swap to determine target chain
  const swap = await ctx.getSwap(id);

  // Check if this is an arkade_to_evm swap (uses coordinator redeemAndExecute)
  if ("direction" in swap && swap.direction === "arkade_to_evm") {
    return buildCoordinatorClaimData(
      id,
      swap as unknown as ArkadeToEvmSwapResponse,
      ctx,
      destination,
    );
  }

  // target_token may be a string (TokenId) or object (TokenInfo with chain + token_id)
  const chain =
    typeof swap.target_token === "object" &&
    swap.target_token !== null &&
    "chain" in swap.target_token
      ? getClaimChainFromChainName(
          (swap.target_token as { chain: string }).chain,
        )
      : getChainFromTokenId(swap.target_token as string);

  if (!chain) {
    return {
      success: false,
      message: `Unknown target chain for token: ${JSON.stringify(swap.target_token)}. Cannot determine claim method.`,
    };
  }

  // Arkade claims return data for manual execution with user's keys
  if (chain === "arkade") {
    // For EVM-to-Arkade swaps, we need specific fields from the swap
    // @ts-expect-error
    const arkadeSwap = swap as {
      sender_pk: string;
      server_pk: string;
      network: string;
      unilateral_claim_delay?: number;
      unilateral_refund_delay?: number;
      unilateral_refund_without_receiver_delay?: number;
      htlc_address_arkade?: string;
      vhtlc_refund_locktime?: number;
    };
    return buildArkadeClaimData(arkadeSwap);
  }

  // EVM claims (Ethereum, Polygon, Arbitrum) return data for manual execution
  return buildEthereumClaimData(id, secret, swap, chain);
}

/**
 * Builds the claim data for an Arkade-to-EVM swap (coordinator redeemAndExecute).
 *
 * The user must:
 * 1. Build the EIP-712 digest using `buildRedeemDigest()` from `evm/coordinator`
 * 2. Sign the digest with their EVM wallet
 * 3. Build the transaction using `encodeRedeemAndExecute()`
 * 4. Submit the transaction to the coordinator contract
 *
 * @param id - The swap ID
 * @param swap - The swap response
 * @param ctx - The redeem context with API client
 * @param destination - Optional EVM address for receiving tokens. If provided and the swap
 *                      involves a DEX swap (target != WBTC), fresh calldata is fetched.
 */
async function buildCoordinatorClaimData(
  id: string,
  swap: ArkadeToEvmSwapResponse,
  ctx: RedeemContext,
  destination?: string,
): Promise<ClaimResult> {
  // WBTC address from server response, fall back to well-known addresses
  const WBTC_BY_CHAIN_ID: Record<number, string> = {
    137: "0x1BFD67037B42Cf73acF2047067bd4F2C47D9BfD6", // Polygon
    1: "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599", // Ethereum
    42161: "0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f", // Arbitrum
  };
  const wbtcAddress = swap.wbtc_address ?? WBTC_BY_CHAIN_ID[swap.evm_chain_id];
  if (!wbtcAddress) {
    return {
      success: false,
      message: `Cannot determine WBTC address for chain ID ${swap.evm_chain_id}`,
    };
  }

  // target_token.token_id contains the ERC-20 contract address for the final token
  const targetTokenAddress = String(swap.target_token.token_id);

  // Check if this swap involves a DEX swap (target token is different from WBTC)
  const needsDexSwap =
    targetTokenAddress.toLowerCase() !== wbtcAddress.toLowerCase();

  let dexCallData: { to: string; data: string; value: string } | undefined;

  // Fetch fresh DEX calldata if destination is provided and swap needs DEX
  if (destination && needsDexSwap) {
    const response = await ctx.apiClient.GET(
      "/swap/{id}/redeem-and-swap-calldata",
      {
        params: {
          path: { id },
          query: { destination },
        },
      },
    );

    if (response.error) {
      return {
        success: false,
        message: `Failed to fetch DEX calldata: ${response.error.error || "Unknown error"}`,
      };
    }

    if (response.data.dex_calldata) {
      dexCallData = {
        to: response.data.dex_calldata.to,
        data: response.data.dex_calldata.data,
        value: response.data.dex_calldata.value,
      };
    }
  }

  const coordinatorClaimData: CoordinatorClaimData = {
    htlcAddress: swap.evm_htlc_address,
    coordinatorAddress: swap.evm_coordinator_address,
    chainId: swap.evm_chain_id,
    amount: swap.evm_expected_sats,
    wbtcAddress,
    sender: swap.server_evm_address,
    timelock: swap.evm_refund_locktime,
    dexCallData,
    targetTokenAddress,
    network: swap.network,
  };

  const message = destination
    ? "Arkade-to-EVM claims require EIP-712 signing. Use buildRedeemDigest() and encodeRedeemAndExecute() with the provided data."
    : "Arkade-to-EVM claims require EIP-712 signing. Note: No destination provided - call claim() again with your EVM address to fetch fresh DEX calldata.";

  return {
    success: true,
    message,
    coordinatorClaimData,
  };
}

/**
 * Builds the claim data for an Arkade swap (EVM-to-Arkade direction).
 *
 * For Arkade claims, the user needs to call `buildArkadeClaim` with their
 * secret key and destination address. This function extracts the necessary
 * parameters from the swap response.
 */
function buildArkadeClaimData(swap: {
  sender_pk: string;
  server_pk: string;
  network: string;
  unilateral_claim_delay?: number;
  unilateral_refund_delay?: number;
  unilateral_refund_without_receiver_delay?: number;
  htlc_address_arkade?: string;
  vhtlc_refund_locktime?: number;
}): ClaimResult {
  if (!swap.htlc_address_arkade) {
    return {
      success: false,
      message: "Swap does not have an Arkade HTLC address.",
      chain: "arkade",
    };
  }

  const arkadeClaimData: ArkadeClaimData = {
    lendaswapPubKey: swap.sender_pk,
    arkadeServerPubKey: swap.server_pk,
    vhtlcAddress: swap.htlc_address_arkade,
    refundLocktime: swap.vhtlc_refund_locktime ?? 0,
    unilateralClaimDelay: swap.unilateral_claim_delay ?? 0,
    unilateralRefundDelay: swap.unilateral_refund_delay ?? 0,
    unilateralRefundWithoutReceiverDelay:
      swap.unilateral_refund_without_receiver_delay ?? 0,
    network: swap.network,
  };

  return {
    success: true,
    message:
      "Arkade claims require your secret key. Use buildArkadeClaim() with the provided data.",
    chain: "arkade",
    arkadeClaimData,
  };
}
