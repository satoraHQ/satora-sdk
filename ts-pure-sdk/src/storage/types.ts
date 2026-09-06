/**
 * Type definitions for swap storage.
 *
 * These types define the schema for persisted swap data.
 * The version field enables schema migrations when the structure changes.
 */

import type { GetSwapResponse } from "../api/client.js";

/** Current schema version for stored swaps */
export const SWAP_STORAGE_VERSION = 2;

/**
 * Extended swap data stored locally.
 *
 * Contains both the API response and client-side parameters needed
 * for claim/refund operations. This matches the Rust SDK's
 * `ExtendedSwapStorageData` pattern.
 *
 * @example
 * ```ts
 * const storedSwap: StoredSwap = {
 *   version: SWAP_STORAGE_VERSION,
 *   swapId: "uuid-here",
 *   keyIndex: 0,
 *   response: await client.getSwap("uuid-here"),
 *   publicKey: "02...",
 *   preimage: "abc123...",
 *   preimageHash: "def456...",
 *   secretKey: "789...",
 *   storedAt: Date.now(),
 *   updatedAt: Date.now(),
 * };
 * ```
 */
export interface StoredSwap {
  /** Schema version for migrations */
  version: number;

  /** The swap ID (primary key) */
  swapId: string;

  /** Key derivation index used for this swap */
  keyIndex: number;

  /** Full API response from GetSwap endpoint */
  response: GetSwapResponse;

  /** Compressed public key (hex-encoded, 33 bytes) */
  publicKey: string;

  /** Preimage for claiming HTLCs (hex-encoded, 32 bytes) */
  preimage: string;

  /** SHA256 hash of preimage / hash lock (hex-encoded, 32 bytes) */
  preimageHash: string;

  /** Secret key for signing refund transactions (hex-encoded, 32 bytes) */
  secretKey: string;

  /** Timestamp when the swap was first stored locally (ms since epoch) */
  storedAt: number;

  /** Timestamp when the swap was last updated locally (ms since epoch) */
  updatedAt: number;

  /** Target address for receiving funds (e.g., BTC address for EVM→Bitcoin swaps) */
  targetAddress?: string;

  /**
   * Pinned CCTP bridge claim recipient for non-EVM destinations (Solana): the
   * destination USDC ATA (base58), resolved at create time. Lets a bare
   * `claim(swapId)` — e.g. the background auto-claim worker — route the burn
   * without the caller re-deriving it.
   */
  bridgeRecipient?: string;

  /**
   * The Solana wallet pubkey owning {@link bridgeRecipient}, pinned only when
   * the ATA still needs creation (`recipientSetup`) — its presence tells the
   * claim to use the ATA-creating forwarder variant, matching the bridge fee
   * committed at create time.
   */
  bridgeRecipientWallet?: string;

  /**
   * The tx this client knows funded an EVM-sourced swap: the one it sent
   * (corrected to the mined hash once the receipt is in) or the relay
   * reported. Kept beside `response` because every server refresh replaces
   * that copy, and the server only learns the txid once its monitor has
   * indexed the funding.
   */
  evmFundTxid?: string;

  /** The HTLCCoordinator the funding went through; pins the refund target. */
  evmCoordinatorAddress?: string;
}
