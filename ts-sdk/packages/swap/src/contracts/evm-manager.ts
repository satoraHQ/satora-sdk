/**
 * The EVM {@link ContractManager} — the stateful I/O adapter that observes
 * `HTLCErc20` swaps across one or more EVM chains.
 *
 * Reading chain state is delegated to an injected {@link EvmChainReader} (one per
 * chainId) rather than a bundled viem/ethers client, so the swap package stays
 * chain-library-agnostic and this adapter is testable against a fake. The pure
 * event→observation mapping lives in `./evm.js`.
 *
 * Unlike Arkade, EVM spans multiple chains with independent `block.timestamp`
 * clocks, so this manager is chain-aware: it routes each ref by `chainId` and
 * {@link chainNow} is ref-scoped.
 */
import type { HtlcObservation } from "../actions/types.js";
import { type EvmHtlcEvent, evmObservation } from "./evm.js";
import {
  type ContractManager,
  type HtlcRef,
  htlcKey,
  type Ledger,
} from "./types.js";

type EvmRef = Extract<HtlcRef, { ledger: "evm" }>;

/** Identifies one HTLC to read: contract + preimageHash (+ the claim guard). */
export type EvmHtlcQuery = {
  htlc: `0x${string}`;
  preimageHash: `0x${string}`;
  /** A `SwapCreated` counts only when it pays this address (term check). */
  claimAddress: `0x${string}`;
  /**
   * The rest of the swap-key tuple, when the swap response exposed it. It makes
   * {@link htlcQueryKey} unique to this HTLC, so two swaps sharing a preimage
   * hash hold separate entries rather than one overwriting the other. It does
   * not change how settlements are attributed — those go by hash either way.
   */
  terms?: EvmHtlcTerms;
};

/**
 * The result key for one query in a batch.
 *
 * `preimageHash` identifies no single HTLC — any number can share one — so the
 * rest of the swap-key tuple is what separates two HTLCs on the same contract.
 * A caller that knows the tuple gets a key unique to its HTLC; one that does not
 * falls back to contract + hash, which is ambiguous by construction — two such
 * callers on one hash share an entry.
 */
export function htlcQueryKey(q: {
  htlc: `0x${string}`;
  preimageHash: `0x${string}`;
  claimAddress?: `0x${string}`;
  terms?: EvmHtlcTerms;
}): string {
  const base = `${q.htlc.toLowerCase()}:${q.preimageHash.toLowerCase()}`;
  if (!q.terms || !q.claimAddress) return base;
  const { amount, token, sender, timelockSec } = q.terms;
  return [
    base,
    amount,
    token.toLowerCase(),
    sender.toLowerCase(),
    q.claimAddress.toLowerCase(),
    timelockSec,
  ].join(":");
}

/** The swap-key tuple fields beyond contract, hash and claim address. */
export type EvmHtlcTerms = {
  amount: bigint;
  token: `0x${string}`;
  sender: `0x${string}`;
  timelockSec: number;
};

/** The batch key for a tracked ref, carrying its terms when the swap exposed them. */
export function refQueryKey(ref: EvmRef): string {
  const full = activeQuery(ref);
  return htlcQueryKey({
    htlc: ref.htlc,
    preimageHash: ref.preimageHash,
    claimAddress: ref.claimAddress,
    terms: full && {
      amount: full.amount,
      token: full.token,
      sender: full.sender,
      timelockSec: full.timelockSec,
    },
  });
}

/**
 * The contract's full swap-key tuple for one HTLC. `isActive` hashes all of it
 * into the swap key, so an `isActive == true` with these EXPECTED values also
 * proves the HTLC was funded on exactly the expected terms.
 */
export type EvmActiveQuery = {
  htlc: `0x${string}`;
  preimageHash: `0x${string}`;
  amount: bigint;
  token: `0x${string}`;
  sender: `0x${string}`;
  claimAddress: `0x${string}`;
  timelockSec: number;
};

/** Reads `HTLCErc20` state for one EVM chain. Implemented over viem/ethers/etc. */
export type EvmChainReader = {
  /**
   * The decoded lifecycle events for every queried HTLC, keyed by
   * {@link htlcQueryKey}. Batched so a whole chain scan costs one RPC request
   * regardless of how many swaps are tracked; a queried HTLC with no events maps
   * to an empty array. `fromBlock` lower-bounds the scan (default genesis) —
   * pass the swaps' creation-block estimate so providers never see an
   * unbounded log query.
   */
  getHtlcEventsBatch(
    queries: EvmHtlcQuery[],
    fromBlock?: bigint,
  ): Promise<Map<string, EvmHtlcEvent[]>>;
  /**
   * Whether each queried HTLC is currently open, keyed by {@link htlcQueryKey}.
   * The cheap routine check (`eth_call`, batched via Multicall3): `true` means
   * funded-and-unspent on exactly the queried terms; `false` is ambiguous
   * (never funded / claimed / refunded) and needs {@link getHtlcEventsBatch}
   * to classify.
   */
  isActiveBatch(queries: EvmActiveQuery[]): Promise<Map<string, boolean>>;
  /** The latest block's timestamp (ms) and number. */
  getLatestBlock(): Promise<{ timeMs: number; number: bigint }>;
};

export type EvmContractManagerDeps = {
  /** A chain reader per EVM `chainId` this manager serves. */
  readers: Map<number, EvmChainReader>;
};

/** A chain clock reading: block.timestamp/number plus when we fetched it. */
type ChainClock = {
  blockTimeMs: number;
  blockNumber: bigint;
  fetchedAtMs: number;
};

/**
 * Rough average block time per supported chain, for estimating "the block
 * around a swap's creation time". Only a LOWER bound for log scans, so being
 * generous (scanning further back) is safe; the ×1.5 factor in the estimate
 * absorbs historical variance. An unlisted chain falls back to a genesis scan.
 */
const AVG_BLOCK_MS: Record<number, number> = {
  1: 12_000,
  137: 2_100,
  42161: 250,
  30: 30_000,
  31: 30_000, // Rootstock testnet: same merge-mined cadence
};

/** Extra blocks to over-scan past the estimate (reorgs + estimate slack). */
const SCAN_MARGIN_BLOCKS = 1_000n;

export class EvmContractManager implements ContractManager {
  readonly ledger: Ledger = "evm";

  readonly #readers: Map<number, EvmChainReader>;
  /** htlcKey → the ref we're tracking. */
  readonly #refs = new Map<string, EvmRef>();
  /** htlcKey → last known observation. */
  readonly #obs = new Map<string, HtlcObservation>();
  /** htlcKey → the preimage a claim revealed. */
  readonly #preimages = new Map<string, `0x${string}`>();
  /** chainId → its last clock reading (extrapolated in {@link chainNow}). */
  readonly #now = new Map<number, ChainClock>();
  readonly #listeners = new Set<
    (ref: HtlcRef, state: HtlcObservation) => void
  >();

  private constructor(deps: EvmContractManagerDeps) {
    this.#readers = deps.readers;
  }

  static fromDeps(deps: EvmContractManagerDeps): EvmContractManager {
    return new EvmContractManager(deps);
  }

  canObserve(ref: HtlcRef): boolean {
    return ref.ledger === "evm" && this.#readers.has(ref.chainId);
  }

  async register(ref: HtlcRef): Promise<void> {
    if (ref.ledger !== "evm")
      throw new Error(`EvmContractManager can't track a '${ref.ledger}' HTLC`);
    // Fail loudly on an unconfigured chain rather than storing a ref we can never
    // observe (its state/clock would stay undefined and the tracker would wait on
    // it forever). The caller must add the chain via ClientBuilder.withEvmRpcUrls().
    if (!this.#readers.has(ref.chainId))
      throw new Error(
        `no EVM reader for chain ${ref.chainId} — configure it via ClientBuilder.withEvmRpcUrls()`,
      );
    this.#refs.set(htlcKey(ref), ref);
  }

  async unregister(ref: HtlcRef): Promise<void> {
    if (ref.ledger !== "evm") return;
    const key = htlcKey(ref);
    this.#refs.delete(key);
    this.#obs.delete(key);
    this.#preimages.delete(key);
    // Drop the chain's state once nothing on it is tracked anymore.
    if (![...this.#refs.values()].some((r) => r.chainId === ref.chainId))
      this.#now.delete(ref.chainId);
  }

  getState(ref: HtlcRef): HtlcObservation | undefined {
    return ref.ledger === "evm" ? this.#obs.get(htlcKey(ref)) : undefined;
  }

  chainNow(ref: HtlcRef): number | undefined {
    if (ref.ledger !== "evm") return undefined;
    const clock = this.#now.get(ref.chainId);
    if (!clock) return undefined;
    // Extrapolate between reads: EVM block.timestamp tracks wall-clock closely,
    // so the clock stays current without polling `getBlock`. This is what lets a
    // timelock flip (e.g. refund unlocking) surface between slow scans; the
    // worker re-verifies against the real chain before ever acting on it.
    return clock.blockTimeMs + Math.max(0, Date.now() - clock.fetchedAtMs);
  }

  onEvent(cb: (ref: HtlcRef, state: HtlcObservation) => void): () => void {
    this.#listeners.add(cb);
    return () => this.#listeners.delete(cb);
  }

  /**
   * Full reconcile of every tracked chain — seeds clocks + observations. Called
   * on demand (tracker start / newly tracked swap), NOT periodically: onward,
   * hints drive targeted `reconcile`s and the tracker's at-risk safety net
   * re-reads only swaps with client funds on the line.
   */
  async refresh(): Promise<void> {
    const chainIds = [
      ...new Set([...this.#refs.values()].map((r) => r.chainId)),
    ];
    // Settled, not all: every EVM chain shares this one manager, so a single
    // endpoint failing would otherwise leave the HTLCs on every OTHER chain
    // unobserved — a swap would stop deriving because an unrelated swap's chain
    // is unreachable. A chain that throws keeps its previous observations and is
    // retried on the next reconcile.
    const results = await Promise.allSettled(
      chainIds.map((c) => this.#reconcileChain(c)),
    );
    results.forEach((result, index) => {
      if (result.status === "rejected")
        console.warn(
          `EVM reconcile failed for chain ${chainIds[index]}:`,
          result.reason,
        );
    });
  }

  /** Targeted verify (hint / pre-action path) — never gated. */
  async reconcile(ref: HtlcRef): Promise<void> {
    if (ref.ledger !== "evm") return;
    const tracked = this.#refs.get(htlcKey(ref));
    if (!tracked) return;
    const reader = this.#readers.get(tracked.chainId);
    if (!reader) return;
    await this.#readClock(reader, tracked.chainId);
    await this.#reconcileRefs(reader, tracked.chainId, [tracked]);
  }

  dispose(): void {
    this.#listeners.clear();
  }

  /** The preimage a claim revealed on this HTLC, if one was seen. */
  getPreimage(ref: HtlcRef): `0x${string}` | undefined {
    return ref.ledger === "evm" ? this.#preimages.get(htlcKey(ref)) : undefined;
  }

  /** Read and store a chain's clock (with its fetch time, for extrapolation). */
  async #readClock(reader: EvmChainReader, chainId: number): Promise<void> {
    const block = await reader.getLatestBlock();
    this.#now.set(chainId, {
      blockTimeMs: block.timeMs,
      blockNumber: block.number,
      fetchedAtMs: Date.now(),
    });
  }

  /** Refresh one chain's clock and re-observe every HTLC tracked on it. */
  async #reconcileChain(chainId: number): Promise<void> {
    const reader = this.#readers.get(chainId);
    if (!reader) return;
    await this.#readClock(reader, chainId);
    const refs = [...this.#refs.values()].filter((r) => r.chainId === chainId);
    await this.#reconcileRefs(reader, chainId, refs);
  }

  /**
   * Re-observe the given refs. Two tiers:
   *
   * 1. Refs with the full contract tuple get the cheap `isActive` check —
   *    `true` proves open-and-on-expected-terms (the swap key hashes them) →
   *    `confirmed`, no log scan at all. This is the common case.
   * 2. Everything else — an incomplete tuple, an inactive-and-unclassified
   *    HTLC (never funded vs claimed vs refunded), or an `isActive` transport
   *    failure (e.g. no Multicall3 on a dev chain) — is classified from one
   *    batched log read, lower-bounded by the swaps' creation-block estimate.
   *
   * A ref already latched on a spend is skipped entirely: terminal per leg,
   * nothing left to learn.
   */
  async #reconcileRefs(
    reader: EvmChainReader,
    chainId: number,
    refs: EvmRef[],
  ): Promise<void> {
    const open = refs.filter((r) => !this.#isSpent(r));
    if (open.length === 0) return;

    const fast = open.filter((r) => activeQuery(r) !== undefined);
    let needLogs = open.filter((r) => activeQuery(r) === undefined);

    if (fast.length > 0) {
      try {
        const active = await reader.isActiveBatch(
          fast.map((r) => activeQuery(r) as EvmActiveQuery),
        );
        for (const ref of fast) {
          if (active.get(refQueryKey(ref)) === true)
            this.#set(htlcKey(ref), "confirmed");
          else needLogs.push(ref); // inactive: classify from logs
        }
      } catch (error) {
        console.warn(
          `EVM isActive check failed for chain ${chainId} — falling back to logs:`,
          error,
        );
        needLogs = open;
      }
    }
    if (needLogs.length === 0) return;

    // Split by whether the creation time is known: known-age refs share a scan
    // lower-bounded near the oldest creation block, unknown-age refs (legacy
    // stored swaps without created_at) get their own full-range scan. One
    // batch for both would force the WORSE bound on everyone — genesis for the
    // whole batch, or (if unknowns were just filtered from the estimate)
    // possibly missing an old ref's events and misreading it as never funded.
    const known = needLogs.filter((r) => r.createdAtMs !== undefined);
    const unknown = needLogs.filter((r) => r.createdAtMs === undefined);
    await Promise.all(
      (
        [
          [known, this.#estimateFromBlock(chainId, known)],
          [unknown, 0n],
        ] as const
      )
        .filter(([refs]) => refs.length > 0)
        .map(([refs, fromBlock]) =>
          this.#classifyFromLogs(reader, refs, fromBlock),
        ),
    );
  }

  /** Read the refs' lifecycle logs (from `fromBlock`) and set observations. */
  async #classifyFromLogs(
    reader: EvmChainReader,
    refs: EvmRef[],
    fromBlock: bigint,
  ): Promise<void> {
    const events = await reader.getHtlcEventsBatch(
      refs.map((r) => {
        const full = activeQuery(r);
        return {
          htlc: r.htlc,
          preimageHash: r.preimageHash,
          claimAddress: r.claimAddress,
          terms: full && {
            amount: full.amount,
            token: full.token,
            sender: full.sender,
            timelockSec: full.timelockSec,
          },
        };
      }),
      fromBlock,
    );
    for (const ref of refs) {
      const { observation, preimage } = evmObservation(
        events.get(refQueryKey(ref)) ?? [],
        { amount: ref.expectedAmount, token: ref.expectedToken },
      );
      const key = htlcKey(ref);
      if (preimage) this.#preimages.set(key, preimage);
      this.#set(key, observation);
    }
  }

  #isSpent(ref: EvmRef): boolean {
    const obs = this.#obs.get(htlcKey(ref));
    return obs === "spent_claim" || obs === "spent_refund";
  }

  /**
   * The block to scan logs from: roughly where the OLDEST of the given swaps
   * was created (an HTLC has no events before its swap existed). Only a lower
   * bound, so estimation errs early — ×1.5 on elapsed time plus a fixed block
   * margin. Falls back to genesis when the creation time, clock, or the
   * chain's block time is unknown.
   */
  #estimateFromBlock(chainId: number, refs: EvmRef[]): bigint {
    const clock = this.#now.get(chainId);
    const avgBlockMs = AVG_BLOCK_MS[chainId];
    const createdAts = refs.map((r) => r.createdAtMs ?? 0);
    const oldestMs = Math.min(...createdAts);
    if (!clock || !avgBlockMs || oldestMs <= 0) return 0n;
    const elapsedMs = Math.max(0, clock.blockTimeMs - oldestMs);
    const blocksBehind =
      BigInt(Math.ceil((elapsedMs / avgBlockMs) * 1.5)) + SCAN_MARGIN_BLOCKS;
    const from = clock.blockNumber - blocksBehind;
    return from > 0n ? from : 0n;
  }

  /** Update an observation and notify on change; never downgrade a resolved spend. */
  #set(key: string, observation: HtlcObservation): void {
    const ref = this.#refs.get(key);
    if (!ref) return;
    const current = this.#obs.get(key);
    if (current === observation) return;
    const spendStates: HtlcObservation[] = ["spent_claim", "spent_refund"];
    if (
      current &&
      spendStates.includes(current) &&
      !spendStates.includes(observation)
    )
      return;
    this.#obs.set(key, observation);
    for (const listener of this.#listeners) listener(ref, observation);
  }
}

/**
 * The complete `isActive` tuple for a ref, or `undefined` when the swap
 * response didn't expose a field (then only the log path can observe the leg).
 */
function activeQuery(ref: EvmRef): EvmActiveQuery | undefined {
  if (
    ref.sender === undefined ||
    ref.expectedToken === undefined ||
    ref.timelockSec === undefined
  )
    return undefined;
  return {
    htlc: ref.htlc,
    preimageHash: ref.preimageHash,
    amount: ref.expectedAmount,
    token: ref.expectedToken,
    sender: ref.sender,
    claimAddress: ref.claimAddress,
    timelockSec: ref.timelockSec,
  };
}
