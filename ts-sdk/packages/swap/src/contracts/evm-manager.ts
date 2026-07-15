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

/** Reads `HTLCErc20` state for one EVM chain. Implemented over viem/ethers/etc. */
export type EvmChainReader = {
  /**
   * The decoded lifecycle events (oldest→newest) for the HTLC identified by
   * `(htlc contract, preimageHash)`.
   */
  getHtlcEvents(
    htlc: `0x${string}`,
    preimageHash: `0x${string}`,
    claimAddress: `0x${string}`,
  ): Promise<EvmHtlcEvent[]>;
  /** The latest block's `block.timestamp`, in ms. */
  getBlockTimeMs(): Promise<number>;
  /** Fire `cb` when new blocks/logs may have changed watched HTLCs; returns unsubscribe. */
  watch(cb: () => void): () => void;
};

export type EvmContractManagerDeps = {
  /** A chain reader per EVM `chainId` this manager serves. */
  readers: Map<number, EvmChainReader>;
};

export class EvmContractManager implements ContractManager {
  readonly ledger: Ledger = "evm";

  readonly #readers: Map<number, EvmChainReader>;
  /** htlcKey → the ref we're tracking. */
  readonly #refs = new Map<string, EvmRef>();
  /** htlcKey → last known observation. */
  readonly #obs = new Map<string, HtlcObservation>();
  /** htlcKey → the preimage a claim revealed. */
  readonly #preimages = new Map<string, `0x${string}`>();
  /** chainId → its latest block time (ms). */
  readonly #now = new Map<number, number>();
  /** chainId → unsubscribe for its (single, shared) watch. */
  readonly #watchUnsubs = new Map<number, () => void>();
  readonly #listeners = new Set<
    (ref: HtlcRef, state: HtlcObservation) => void
  >();

  private constructor(deps: EvmContractManagerDeps) {
    this.#readers = deps.readers;
  }

  static fromDeps(deps: EvmContractManagerDeps): EvmContractManager {
    return new EvmContractManager(deps);
  }

  async register(ref: HtlcRef): Promise<void> {
    if (ref.ledger !== "evm")
      throw new Error(`EvmContractManager can't track a '${ref.ledger}' HTLC`);
    this.#refs.set(htlcKey(ref), ref);
    this.#ensureWatch(ref.chainId);
    await this.#reconcileChain(ref.chainId);
  }

  async unregister(ref: HtlcRef): Promise<void> {
    if (ref.ledger !== "evm") return;
    const key = htlcKey(ref);
    this.#refs.delete(key);
    this.#obs.delete(key);
    this.#preimages.delete(key);
    // Drop the chain's watch once nothing on it is tracked anymore.
    if (![...this.#refs.values()].some((r) => r.chainId === ref.chainId)) {
      this.#watchUnsubs.get(ref.chainId)?.();
      this.#watchUnsubs.delete(ref.chainId);
      this.#now.delete(ref.chainId);
    }
  }

  getState(ref: HtlcRef): HtlcObservation | undefined {
    return ref.ledger === "evm" ? this.#obs.get(htlcKey(ref)) : undefined;
  }

  chainNow(ref: HtlcRef): number | undefined {
    return ref.ledger === "evm" ? this.#now.get(ref.chainId) : undefined;
  }

  onEvent(cb: (ref: HtlcRef, state: HtlcObservation) => void): () => void {
    this.#listeners.add(cb);
    return () => this.#listeners.delete(cb);
  }

  async refresh(): Promise<void> {
    const chainIds = new Set([...this.#refs.values()].map((r) => r.chainId));
    await Promise.all([...chainIds].map((c) => this.#reconcileChain(c)));
  }

  dispose(): void {
    for (const unsub of this.#watchUnsubs.values()) unsub();
    this.#watchUnsubs.clear();
    this.#listeners.clear();
  }

  /** The preimage a claim revealed on this HTLC, if one was seen. */
  getPreimage(ref: HtlcRef): `0x${string}` | undefined {
    return ref.ledger === "evm" ? this.#preimages.get(htlcKey(ref)) : undefined;
  }

  #ensureWatch(chainId: number): void {
    if (this.#watchUnsubs.has(chainId)) return;
    const reader = this.#readers.get(chainId);
    if (!reader) return;
    this.#watchUnsubs.set(
      chainId,
      reader.watch(() => void this.#reconcileChain(chainId)),
    );
  }

  /** Refresh one chain's clock and re-observe every HTLC tracked on it. */
  async #reconcileChain(chainId: number): Promise<void> {
    const reader = this.#readers.get(chainId);
    if (!reader) return;
    this.#now.set(chainId, await reader.getBlockTimeMs());
    await Promise.all(
      [...this.#refs.values()]
        .filter((r) => r.chainId === chainId)
        .map((r) => this.#reconcileRef(reader, r)),
    );
  }

  async #reconcileRef(reader: EvmChainReader, ref: EvmRef): Promise<void> {
    const events = await reader.getHtlcEvents(
      ref.htlc,
      ref.preimageHash,
      ref.claimAddress,
    );
    const { observation, preimage } = evmObservation(events, {
      amount: ref.expectedAmount,
      token: ref.expectedToken,
    });
    const key = htlcKey(ref);
    if (preimage) this.#preimages.set(key, preimage);
    this.#set(key, observation);
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
