/**
 * Gasless claim logic.
 *
 * Claims are submitted via the server's claim-gasless endpoint which
 * uses the HTLCCoordinator contract for gasless execution.
 */

import type {
  ArkadeToEvmSwapResponse,
  LightningToEvmSwapResponse,
} from "../api/client.js";
import { buildRedeemDigest, signEvmDigest } from "../evm/index.js";
import type { ClaimGaslessResult } from "./types.js";

/** Swap types that support gasless claiming */
export type GaslessSwapResponse =
  | ArkadeToEvmSwapResponse
  | LightningToEvmSwapResponse;

/** Parameters for a gasless claim */
export interface GaslessClaimParams {
  /** Base URL for the API (e.g. "https://api.lendaswap.com") */
  baseUrl: string;
  /** The swap preimage/secret (hex, with or without 0x prefix) */
  preimage: string;
  /** The secret key for EVM signing (raw bytes) */
  secretKey: Uint8Array;
  /** The swap data from the server */
  swap: GaslessSwapResponse;
  /** The EVM address where tokens should be sent */
  destination: string;
  /** Pre-fetched DEX calldata (for non-WBTC targets) */
  dexCalldata?: { to: string; data: string; value: string };
}

/**
 * Claims an Arkade-to-EVM swap gaslessly via the server.
 *
 * Builds the EIP-712 digest, signs it with the provided secret key,
 * and sends the signature + secret to the server. The server submits
 * the `coordinator.redeemAndExecute` transaction.
 *
 * @param params - All data needed for the gasless claim.
 * @returns The gasless claim result with transaction hash.
 */
export async function claimViaGasless(
  params: GaslessClaimParams,
): Promise<ClaimGaslessResult> {
  const { baseUrl, preimage, secretKey, swap, destination, dexCalldata } =
    params;

  const secretHex = preimage.startsWith("0x") ? preimage : `0x${preimage}`;

  const wbtcAddress = swap.wbtc_address;
  const amount = BigInt(swap.evm_expected_sats);

  // target_token.token_id contains the ERC-20 contract address for the final token
  const targetTokenAddress = String(swap.target_token.token_id);

  // Check if target token differs from WBTC (meaning a DEX swap is needed)
  const needsDexSwap =
    targetTokenAddress.toLowerCase() !== wbtcAddress.toLowerCase();

  // sweepToken: if there's a DEX swap, sweep the target token; otherwise sweep WBTC
  const sweepToken = needsDexSwap ? targetTokenAddress : wbtcAddress;

  // Build EIP-712 digest
  const digest = buildRedeemDigest({
    htlcAddress: swap.evm_htlc_address,
    chainId: swap.evm_chain_id,
    preimage: secretHex,
    amount,
    token: wbtcAddress,
    sender: swap.server_evm_address,
    timelock: swap.evm_refund_locktime,
    caller: swap.evm_coordinator_address,
    destination,
    sweepToken,
    // TODO: this is the slippage protection. I guess it shouldn't be 0
    minAmountOut: 0n,
  });

  // Sign with the swap's internally derived EVM key
  const sig = signEvmDigest(secretKey, digest);

  // Send to server with DEX calldata if applicable
  const response = await fetch(`${baseUrl}/swap/${swap.id}/claim-gasless`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      secret: secretHex,
      destination,
      v: sig.v,
      r: sig.r,
      s: sig.s,
      dex_calldata: needsDexSwap ? dexCalldata : undefined,
    }),
  });

  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(`Gasless claim failed (${response.status}): ${errorText}`);
  }

  const result = await response.json();
  return {
    id: result.id,
    status: result.status,
    txHash: result.tx_hash,
    message: result.message,
  };
}
