/**
 * Gasless claim logic.
 *
 * Claims are submitted via the server's claim-gasless endpoint which
 * uses the HTLCCoordinator contract for gasless execution.
 */

import type { ArkadeToEvmSwapResponse } from "../api/client.js";
import {
  buildNativeRedeemDigest,
  buildRedeemDigest,
  computeCoordinatorCallsHash,
  NATIVE_TOKEN_ADDRESS,
  signEvmDigest,
} from "../evm/index.js";
import { PROTOCOL_HEADERS } from "../protocol.js";
import { CLIENT_AGENT } from "../version.js";
import type { ClaimGaslessResult } from "./types.js";

/** Swap types that support gasless claiming */
export type GaslessSwapResponse = ArkadeToEvmSwapResponse;

/** Parameters for a gasless claim */
export interface GaslessClaimParams {
  /** Base URL for the API (e.g. "https://api.satora.io") */
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
  /** Minimum amount of sweepToken to receive, from the redeem-and-swap-calldata endpoint. */
  minAmountOut: bigint;
  /** keccak256(abi.encode(calls)) from the redeem-and-swap-calldata endpoint.
   *  Required for non-WBTC targets (when dexCalldata is provided). */
  callsHash: string;
  /**
   * Optional non-EVM bridge recipient (e.g. a Solana base58 SPL pubkey).
   * Required when the swap's `bridge_target_chain` is non-EVM. Must
   * match the value passed to `redeem-and-swap-calldata` so the
   * rebuilt server-side `calls_hash` matches the EIP-712 signature.
   *
   * For Solana: this is the recipient's USDC ATA, not the wallet pubkey.
   */
  bridgeRecipient?: string;
  /**
   * Optional Solana wallet pubkey, supplied alongside `bridgeRecipient`
   * when the recipient's USDC ATA does not yet exist. Triggers the
   * extended 65-byte forwarding hookData. Must mirror what was passed
   * to the calldata-fetch endpoint so the rebuilt `calls_hash` matches.
   */
  bridgeRecipientWallet?: string;
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
  const {
    baseUrl,
    preimage,
    secretKey,
    swap,
    destination,
    dexCalldata,
    minAmountOut,
    callsHash,
    bridgeRecipient,
    bridgeRecipientWallet,
  } = params;

  const secretHex = preimage.startsWith("0x") ? preimage : `0x${preimage}`;

  const wbtcAddress = swap.wbtc_address;
  const amount = BigInt(swap.evm_expected_sats);

  // A native-coin lock (`evm_htlc_kind: "native"`, e.g. RBTC on Rootstock)
  // has one claim shape, fixed by the server's relay: no calls, the coin
  // swept to `destination`, the full amount as the floor. The calldata
  // endpoint's DEX fields do not apply; only the digest's domain differs.
  if (nativeHtlcKind(swap)) {
    const digest = buildNativeRedeemDigest({
      htlcAddress: swap.evm_htlc_address,
      chainId: swap.evm_chain_id,
      preimage: secretHex,
      amount,
      sender: swap.server_evm_address,
      timelock: swap.evm_refund_locktime,
      caller: swap.evm_coordinator_address,
      destination,
      sweepToken: NATIVE_TOKEN_ADDRESS,
      minAmountOut: amount,
      callsHash: computeCoordinatorCallsHash([]),
      htlcVersion: swap.evm_htlc_version,
    });
    const sig = signEvmDigest(secretKey, digest);
    return postClaim(baseUrl, swap.id, {
      secret: secretHex,
      destination,
      v: sig.v,
      r: sig.r,
      s: sig.s,
    });
  }

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
    minAmountOut,
    callsHash,
    htlcVersion: swap.evm_htlc_version,
  });

  // Sign with the swap's internally derived EVM key
  const sig = signEvmDigest(secretKey, digest);

  // Send to server with DEX calldata if applicable
  return postClaim(baseUrl, swap.id, {
    secret: secretHex,
    destination,
    v: sig.v,
    r: sig.r,
    s: sig.s,
    dex_calldata: needsDexSwap ? dexCalldata : undefined,
    bridge_recipient: bridgeRecipient,
    bridge_recipient_wallet: bridgeRecipientWallet,
  });
}

/**
 * Whether the swap's lock is the native-coin HTLC family. The field is
 * reported on the responses of the directions that can lock native coin;
 * absent means the ERC-20 family.
 */
function nativeHtlcKind(swap: GaslessSwapResponse): boolean {
  return (swap as { evm_htlc_kind?: string }).evm_htlc_kind === "native";
}

/**
 * The post-claim state named by the server's "wrong state" rejection, or
 * `client_redeeming` for its "claim already in progress" rejection (a relay
 * is in flight and will move the swap there); `undefined` when the rejection
 * is about something else.
 */
export function alreadyClaimedStatus(errorText: string): string | undefined {
  if (/gasless claim already in progress/.test(errorText)) {
    return "client_redeeming";
  }
  const match =
    /cannot claim a swap in (client_redeeming|client_redeemed|server_redeemed)/.exec(
      errorText,
    );
  return match?.[1];
}

async function postClaim(
  baseUrl: string,
  swapId: string,
  body: Record<string, unknown>,
): Promise<ClaimGaslessResult> {
  const response = await fetch(`${baseUrl}/swap/${swapId}/claim-gasless`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-Lendaswap-Client": CLIENT_AGENT,
      ...PROTOCOL_HEADERS,
    },
    body: JSON.stringify(body),
  });

  if (!response.ok) {
    const errorText = await response.text();
    // The server already relayed a claim for this swap (this call, or the
    // auto-claim worker's, or an earlier attempt), or is relaying one right
    // now: the claim is on its way from the client's side, not failed.
    // Retrying would only be refused again.
    const claimed = alreadyClaimedStatus(errorText);
    if (response.status === 400 && claimed) {
      return {
        id: swapId,
        status: claimed,
        txHash: "",
        message: `Swap already claimed (${claimed})`,
      };
    }
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
