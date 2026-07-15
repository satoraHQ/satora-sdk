/**
 * The contract-monitoring layer — modeled on @arkade-os/sdk's
 * `ContractManager`.
 *
 * A {@link ContractManager} is the per-ledger, stateful I/O layer that watches a
 * set of HTLCs and maps their on-chain state to {@link HtlcObservation}. The pure
 * pipeline (`deriveSwapStatus` → `deriveSwapActions`) consumes the observations
 * it produces; a `SwapTracker` (built later) combines the two ledgers of each
 * swap and notifies subscribers of the next action.
 */
import type { HtlcObservation } from "../actions/types.js";

/** Ledgers a swap HTLC can live on. */
export type Ledger = "arkade" | "bitcoin" | "evm" | "lightning";

/**
 * Identifies and parameterises one HTLC on its ledger — enough for a
 * {@link ContractManager} to observe it. Sourced from the swap's recovery
 * bundle. Discriminated by `ledger`.
 */
export type HtlcRef =
  | {
      ledger: "arkade";
      /** VHTLC pkScript (hex) — the contract's unique id. */
      script: string;
      address: string;
      /** SHA256 of the preimage, to tell a claim spend from a refund spend. */
      preimageHash: string;
      /**
       * Expected funding amount in sats. A funding below this is `invalid` (not on
       * the swap's terms) rather than `confirmed` — so the client never claims a
       * server leg the server short-funded.
       */
      expectedSats: number;
      /**
       * Serialized VHTLC parameters (`@arkade-os/sdk`'s `Contract.params`) — the
       * sender/receiver/server keys, preimage hash, and timelocks needed to
       * register the contract for watching. Opaque here: the Client serializes
       * it from the swap's recovery bundle so this layer stays VHTLC-agnostic.
       */
      params: Record<string, string>;
    }
  | {
      ledger: "bitcoin";
      /** HTLC witness-script address — the contract's unique id. */
      address: string;
      preimageHash: string;
      /** Expected funding amount in sats; a funding below this is `invalid`. */
      expectedSats: number;
    }
  | {
      ledger: "evm";
      chainId: number;
      /** The HTLCErc20 contract holding this leg — where its events are emitted. */
      htlc: `0x${string}`;
      /**
       * SHA256 preimage hash (`0x`-prefixed) — the indexed topic that identifies
       * this swap's HTLC in the contract's `SwapCreated`/`SwapRedeemed`/
       * `SwapRefunded` events. Serves as the contract's unique id here.
       */
      preimageHash: `0x${string}`;
      /**
       * The address that can claim this HTLC — the second indexed topic used to
       * filter `SwapCreated`, so only the HTLC actually claimable on the swap's
       * terms is observed (an HTLC with our hash but a different recipient is
       * never seen). Client's EVM address for a server-funded leg; the server's
       * for the client's own leg.
       */
      claimAddress: `0x${string}`;
      /**
       * Expected locked amount (token's smallest unit). A `SwapCreated` below this
       * → `invalid`, so the client never claims a short-funded leg.
       */
      expectedAmount: bigint;
      /** Expected token address; the funding is `invalid` if it locks another token. */
      expectedToken?: `0x${string}`;
    }
  | {
      ledger: "lightning";
      /** Payment hash — the contract's unique id. */
      paymentHash: string;
    };

/** Stable, cross-ledger unique key for an {@link HtlcRef}. */
export function htlcKey(ref: HtlcRef): string {
  switch (ref.ledger) {
    case "arkade":
      return `arkade:${ref.script}`;
    case "bitcoin":
      return `bitcoin:${ref.address}`;
    case "evm":
      return `evm:${ref.chainId}:${ref.preimageHash}`;
    case "lightning":
      return `lightning:${ref.paymentHash}`;
  }
}

/**
 * Per-ledger contract monitor. Implementations keep an internal, event-driven
 * cache: `register` seeds it, `onEvent` pushes changes, `getState` reads it
 * synchronously, `dispose` tears down subscriptions/loops.
 *
 * One manager exists per ledger actually in use and watches every tracked HTLC
 * on that ledger at once (one connection, not one per HTLC).
 */
export interface ContractManager {
  /** The ledger this manager observes. */
  readonly ledger: Ledger;

  /** Start tracking an HTLC (idempotent); seeds its state from the ledger. */
  register(ref: HtlcRef): Promise<void>;

  /** Stop tracking an HTLC. */
  unregister(ref: HtlcRef): Promise<void>;

  /**
   * Current known observation for a tracked HTLC, or `undefined` if it isn't
   * tracked or its state isn't known yet.
   */
  getState(ref: HtlcRef): HtlcObservation | undefined;

  /**
   * The current clock (ms) for evaluating this HTLC's timelock — MTP for
   * Bitcoin/Arkade, `block.timestamp` for EVM. Ref-scoped because one ledger can
   * span several chains with independent clocks (EVM chains); managers whose
   * ledger has a single clock (Arkade) ignore the ref. `undefined` until known.
   */
  chainNow(ref: HtlcRef): number | undefined;

  /** Subscribe to observation changes for any tracked HTLC. Returns unsubscribe. */
  onEvent(cb: (ref: HtlcRef, state: HtlcObservation) => void): () => void;

  /** Force a reconcile from the ledger. */
  refresh(): Promise<void>;

  /** Tear down subscriptions and background work. */
  dispose(): void;
}
