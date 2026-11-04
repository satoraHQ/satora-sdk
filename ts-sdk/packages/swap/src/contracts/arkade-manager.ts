/**
 * The Arkade {@link ContractManager} — the stateful I/O adapter that observes a
 * swap's VHTLC via the Ark indexer.
 *
 * We deliberately do NOT use `@arkade-os/sdk`'s `ContractManager`: its vtxo
 * annotation calls `script.forfeit()` on every contract, but a VHTLC script has
 * no `forfeit()` leaf (it exposes `claim()`/`refund()`), so watching a VHTLC
 * through it throws. Instead we read vtxos directly by pkScript — exactly what
 * the legacy SDK's claim/refund paths do — and map them with the pure helpers in
 * `./arkade.js`.
 *
 * Observation is on-demand: {@link refresh} (tracker start / newly tracked
 * swap) and targeted {@link reconcile}s (server hints, the tracker's at-risk
 * safety net). There is no standing subscription or periodic poll.
 */
import { RestIndexerProvider, type VirtualCoin } from "@arkade-os/sdk";
import { hex } from "@scure/base";
import type { HtlcObservation } from "../actions/types.js";
import type { VirtualTxSource } from "./arkade.js";
import { arkadeObservation, fetchArkadeSpend } from "./arkade.js";
import type { ContractManager, HtlcRef, Ledger } from "./types.js";

/**
 * The Ark indexer surface the observer needs (RestIndexerProvider satisfies it).
 */
export type ArkadeIndexer = VirtualTxSource & {
  getVtxos(opts: {
    scripts: string[];
    spendableOnly?: boolean;
  }): Promise<{ vtxos: VirtualCoin[] }>;
};

export type ArkadeCreateConfig = {
  /** Ark server base URL — used for the indexer. */
  serverUrl: string;
  /**
   * The current Bitcoin MTP (ms) for evaluating VHTLC timelocks. Typically
   * `async () => (await client.getMtp()).mtp * 1000`.
   */
  chainTime?: () => Promise<number>;
};

export type ArkadeContractManagerDeps = {
  indexer: ArkadeIndexer;
  /**
   * The current Bitcoin MTP (ms) — arkade CLTV timelocks are evaluated against
   * Bitcoin's clock, not the Ark server's. Optional: until wired, `chainNow()`
   * reports `undefined` and the tracker holds swaps as provisional.
   */
  chainTime?: () => Promise<number>;
};

/** True when a vtxo is a live funding of the contract (not yet spent). */
function isFunded(vtxo: VirtualCoin): boolean {
  return !vtxo.isSpent && !vtxo.isSwept;
}

/**
 * The offchain txid that spent this contract's VHTLC, if any. Mirrors the backend
 * watcher (`unified_watcher.rs`), which keys spend detection on `arkTxId` — the
 * offchain Arkade tx whose input[0] condition witness carries the revealed
 * preimage. Falls back to `spentBy` (the checkpoint tx, which also reveals it) so
 * a spend is caught under either field: reading only `spentBy` silently missed
 * spent vtxos that expose the spend solely via `arkTxId`, leaving them classified
 * as `confirmed` forever (never emitting spent_claim/spent_refund).
 */
function spendTxid(vtxos: VirtualCoin[]): string | undefined {
  const spent = vtxos.find((v) => v.arkTxId || v.spentBy || v.isSpent);
  return spent?.arkTxId ?? spent?.spentBy;
}

export class ArkadeContractManager implements ContractManager {
  readonly ledger: Ledger = "arkade";

  readonly #indexer: ArkadeIndexer;
  readonly #chainTime?: () => Promise<number>;

  /** pkScript → the ref we're tracking. */
  readonly #refs = new Map<string, Extract<HtlcRef, { ledger: "arkade" }>>();
  /** pkScript → last known observation. */
  readonly #obs = new Map<string, HtlcObservation>();
  /** pkScript → the verified preimage recovered from a claim spend. */
  readonly #preimages = new Map<string, Uint8Array>();
  readonly #listeners = new Set<
    (ref: HtlcRef, state: HtlcObservation) => void
  >();

  /** Last MTP reading (ms) + when it was fetched, for extrapolation. */
  #now: { mtpMs: number; fetchedAtMs: number } | undefined;

  private constructor(deps: ArkadeContractManagerDeps) {
    this.#indexer = deps.indexer;
    this.#chainTime = deps.chainTime;
  }

  static fromDeps(deps: ArkadeContractManagerDeps): ArkadeContractManager {
    return new ArkadeContractManager(deps);
  }

  /** Construct a manager backed by a `RestIndexerProvider` over the Ark server. */
  static async create(
    config: ArkadeCreateConfig,
  ): Promise<ArkadeContractManager> {
    const indexer = new RestIndexerProvider(config.serverUrl);
    return ArkadeContractManager.fromDeps({
      indexer,
      chainTime: config.chainTime,
    });
  }

  canObserve(ref: HtlcRef): boolean {
    // The Arkade server is always configured (default or override), so any Arkade
    // ref is observable.
    return ref.ledger === "arkade";
  }

  async register(ref: HtlcRef): Promise<void> {
    if (ref.ledger !== "arkade")
      throw new Error(
        `ArkadeContractManager can't track a '${ref.ledger}' HTLC`,
      );
    this.#refs.set(ref.script, ref);
  }

  async unregister(ref: HtlcRef): Promise<void> {
    if (ref.ledger !== "arkade") return;
    this.#refs.delete(ref.script);
    this.#obs.delete(ref.script);
    this.#preimages.delete(ref.script);
  }

  getState(ref: HtlcRef): HtlcObservation | undefined {
    return ref.ledger === "arkade" ? this.#obs.get(ref.script) : undefined;
  }

  chainNow(_ref: HtlcRef): number | undefined {
    // Arkade CLTV timelocks share one clock (Bitcoin MTP), so the ref is unused.
    if (!this.#now) return undefined;
    // Extrapolate between reads: MTP advances with wall-clock (lagging it by a
    // roughly constant margin), so the clock stays current without polling.
    // The worker re-verifies on-chain before acting on any flip this causes.
    return this.#now.mtpMs + Math.max(0, Date.now() - this.#now.fetchedAtMs);
  }

  onEvent(cb: (ref: HtlcRef, state: HtlcObservation) => void): () => void {
    this.#listeners.add(cb);
    return () => this.#listeners.delete(cb);
  }

  /**
   * Full reconcile of every tracked ref (clock + vtxos). Called on demand
   * (tracker start / newly tracked swap), NOT periodically: onward, hints and
   * the tracker's at-risk safety net drive targeted `reconcile`s.
   */
  async refresh(): Promise<void> {
    await this.#scanAll();
  }

  /** Targeted verify (hint / pre-action path) — never gated. */
  async reconcile(ref: HtlcRef): Promise<void> {
    if (ref.ledger !== "arkade") return;
    const tracked = this.#refs.get(ref.script);
    if (!tracked) return;
    await this.#readClock();
    await this.#reconcileRef(tracked);
  }

  dispose(): void {
    this.#listeners.clear();
  }

  /** The verified preimage recovered from a claim spend, if one was seen. */
  getPreimage(ref: HtlcRef): Uint8Array | undefined {
    return ref.ledger === "arkade"
      ? this.#preimages.get(ref.script)
      : undefined;
  }

  /** Read and store the MTP clock (with fetch time, for extrapolation). */
  async #readClock(): Promise<void> {
    if (!this.#chainTime) return;
    this.#now = { mtpMs: await this.#chainTime(), fetchedAtMs: Date.now() };
  }

  /** Full scan of every tracked ref (clock + vtxos). */
  async #scanAll(): Promise<void> {
    await this.#readClock();
    await Promise.all(
      [...this.#refs.values()].map((ref) => this.#reconcileRef(ref)),
    );
  }

  /** Read the VHTLC's vtxos and map them to an observation. */
  async #reconcileRef(
    ref: Extract<HtlcRef, { ledger: "arkade" }>,
  ): Promise<void> {
    const { vtxos } = await this.#indexer.getVtxos({ scripts: [ref.script] });
    const spend = spendTxid(vtxos);
    if (spend) {
      const resolved = await fetchArkadeSpend(
        this.#indexer,
        spend,
        hex.decode(ref.preimageHash),
      );
      if (resolved) {
        if (resolved.spend === "claim")
          this.#preimages.set(ref.script, resolved.preimage);
        this.#set(
          ref.script,
          arkadeObservation({ funded: true, spend: resolved.spend }),
        );
        return;
      }
    }
    const funded = vtxos.filter(isFunded);
    const total = funded.reduce((sum, vtxo) => sum + vtxo.value, 0);
    this.#set(
      ref.script,
      arkadeObservation({
        funded: funded.length > 0,
        sufficient: total >= ref.expectedSats,
      }),
    );
  }

  /** Update an observation and notify on change; never downgrade a resolved spend. */
  #set(script: string, observation: HtlcObservation): void {
    const ref = this.#refs.get(script);
    if (!ref) return;
    const current = this.#obs.get(script);
    if (current === observation) return;
    // A resolved spend is terminal for the HTLC — a later funded/absent poll
    // (e.g. spent vtxos dropping out of the listing) must not undo it.
    const spendStates: HtlcObservation[] = ["spent_claim", "spent_refund"];
    if (
      current &&
      spendStates.includes(current) &&
      !spendStates.includes(observation)
    )
      return;
    this.#obs.set(script, observation);
    for (const listener of this.#listeners) listener(ref, observation);
  }
}
