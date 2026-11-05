/**
 * Type definitions for swap storage.
 *
 * These types define the schema for persisted swap data.
 * The version field enables schema migrations when the structure changes.
 */

import type { GetSwapResponse } from "../api/client.js";
import type { EvmFundingTerms } from "../evm/funding-pins.js";

/** Current schema version for stored swaps */
export const SWAP_STORAGE_VERSION = 2;

/** One entry of {@link StoredSwap.evmRehashes}. */
export interface EvmRehashRecord {
  /**
   * The `rehash` transaction. Empty while a sponsored move is sent but not
   * yet seen included; it is then followed only through the receipt its
   * {@link userOpHash} leads to, never inferred from the lock's state.
   */
  txid: string;
  /** The sponsored UserOp that carries the move, hashed before it is sent. */
  userOpHash?: string;
  /** The hash the lock was under before this move (0x-prefixed hex). */
  fromHashLock: string;
  /** The payment hash of the new invoice (0x-prefixed hex). */
  hashLock: string;
  /** The timelock the lock continues under (unix seconds). */
  timelock: number;
}

/**
 * The payment hash an EVM → Lightning swap's lock must use, fixed by this
 * client when it created the swap. See {@link StoredSwap.lightningPaymentHash}.
 */
export interface LightningPaymentHash {
  /** The payment hash (0x-prefixed hex). */
  hash: string;
  /**
   * `invoice`: read from the invoice the caller supplied, so the server had
   * no say in it. `server-invoice`: the swap was created from a Lightning
   * address or LNURL, which the server resolves; the hash was read from
   * the invoice it returned (checked against its `hash_lock` and payout
   * amount), so it is only as honest as that resolution.
   */
  source: "invoice" | "server-invoice";
}

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
   * (corrected to the mined hash once the receipt is in), the relay
   * reported, or one a refund found through the server's copy. Kept beside
   * `response` because every server refresh replaces that copy, and the
   * server only learns the txid once its monitor has indexed the funding.
   * Since a refund may fill it from the server, it is only a hint: a rehash
   * trusts a funding receipt for what it shows under
   * {@link lightningPaymentHash}, whichever tx it came from.
   */
  evmFundTxid?: string;

  /**
   * For an EVM → Lightning swap, the payment hash this client fixed at
   * creation. Every funding path refuses to lock under any other hash (the
   * server could otherwise pick one whose preimage it knows and claim
   * without paying), and a rehash only moves a lock created under it. Kept
   * beside `response`, which every server refresh replaces; a swap without
   * it (recovered, or created by an older SDK) can be refunded but neither
   * funded nor retried. EVM → Bitcoin / Arkade swaps need no such field:
   * their hash is {@link preimageHash}, which this client generates.
   */
  lightningPaymentHash?: LightningPaymentHash;

  /**
   * For an EVM-sourced swap, the funding terms read from its creation
   * response: the coordinator (the only Permit2 spender this client signs
   * for), HTLC, claimer, timelock, tokens and input. Every funding payload
   * must match them and is signed from them; never updated from a later
   * server answer. A swap without it (recovered, or created by an older
   * SDK) can be refunded but not funded.
   */
  evmFundingTerms?: EvmFundingTerms;

  /** The HTLCCoordinator the funding went through; pins the refund target. */
  evmCoordinatorAddress?: string;

  /**
   * The rehashes this client sent for an EVM → Lightning swap, oldest
   * first: each moves the lock from `fromHashLock` to `hashLock` /
   * `timelock` in `txid`. Kept beside `response` because the server's copy
   * switches hash and funding tx once its monitor confirms a move, while a
   * refund is built from the funding tx's `SwapCreated` under the first
   * entry's `fromHashLock`, followed through each `SwapRehashed`.
   */
  evmRehashes?: EvmRehashRecord[];

  /**
   * Source-chain CCTP burn of a CCTP-inbound swap, recorded as soon as it is
   * broadcast. The USDC has left the source chain from then on: `fundSwap`
   * resumes the Arbitrum settlement from it instead of burning again, and
   * `recoverCctpInboundSwap` recovers the USDC with it if settlement fails.
   */
  cctpBurnTxid?: string;

  /**
   * The last block height at which a Solana {@link cctpBurnTxid} can land.
   * Resuming gives the burn up only once the chain is past it, so an RPC that
   * cannot find an older burn never causes a second one.
   */
  cctpBurnLastValidBlockHeight?: number;
}
