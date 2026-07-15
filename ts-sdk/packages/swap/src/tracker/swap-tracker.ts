/**
 * The reactive orchestration.
 *
 * A {@link SwapTracker} watches each tracked swap's on-chain HTLC leg(s) via
 * per-ledger {@link ContractManager}s (two legs for chain↔chain swaps, one for
 * Lightning). On any change it recomputes the next action
 * (`deriveSwapStatus` → `deriveSwapActions`), and — deduped — notifies
 * subscribers, dropping a swap once it reaches a terminal state.
 *
 * This is the observe-mode layer: it *notifies*. Auto-execution (recover/auto
 * mode running the action) is a policy layer built on top of these same
 * notifications, gated by each action's `automation`.
 */
import { deriveSwapActions } from "../actions/derive.js";
import { deriveSwapStatus } from "../actions/status.js";
import type { HtlcObservation, SwapActions } from "../actions/types.js";
import type { ContractManager, HtlcRef, Ledger } from "../contracts/types.js";

/**
 * A swap the tracker watches: its HTLC legs + refund locktimes (from the recovery
 * bundle). Chain↔chain swaps have both legs; Lightning swaps have exactly one
 * on-chain leg (the other side is an off-chain Lightning payment) — see
 * {@link SwapObservations}.
 */
export type TrackedSwap = {
  swapId: string;
  clientHtlc?: HtlcRef;
  serverHtlc?: HtlcRef;
  clientRefundLocktime: number;
  serverRefundLocktime: number;
};

export type ActionSubscriber = (swapId: string, actions: SwapActions) => void;

export type SwapTrackerOptions = {
  /**
   * How often (ms) to re-poll every manager to advance clocks and reconcile
   * observations. Needed because some ledgers (Arkade) have no event push, so
   * state only moves on a poll. `0` disables the timer (tests drive `refresh`).
   */
  refreshIntervalMs?: number;
};

export class SwapTracker {
  readonly #managers: Map<Ledger, ContractManager>;
  readonly #swaps = new Map<string, TrackedSwap>();
  /** Last actions emitted per swap — for dedupe and the subscribe-time snapshot. */
  readonly #lastActions = new Map<string, SwapActions>();
  readonly #subscribers = new Set<ActionSubscriber>();
  readonly #refreshIntervalMs: number;
  #eventUnsubs: Array<() => void> = [];
  #timer: ReturnType<typeof setInterval> | undefined;

  constructor(
    managers: Map<Ledger, ContractManager>,
    options?: SwapTrackerOptions,
  ) {
    this.#managers = managers;
    this.#refreshIntervalMs = options?.refreshIntervalMs ?? 0;
  }

  /** Register each swap's on-chain leg(s), subscribe to manager events, and seed state. */
  async startTracking(swaps: TrackedSwap[]): Promise<void> {
    for (const swap of swaps) {
      this.#swaps.set(swap.swapId, swap);
      for (const leg of legsOf(swap)) await this.#managerFor(leg).register(leg);
    }
    for (const manager of new Set(this.#managers.values())) {
      this.#eventUnsubs.push(manager.onEvent(() => this.#recomputeAll()));
    }
    // Prime each manager: seeds its clock and does a full reconcile, so the first
    // recompute has both observations AND clocks. Without this a ledger whose
    // clock is only populated on refresh (Arkade's MTP) stays `undefined` and the
    // recompute bails forever — nothing is ever emitted.
    await this.#refreshManagers();
    this.#recomputeAll();
    // Poll onward: Arkade has no event push, and clocks advance with wall time.
    if (this.#refreshIntervalMs > 0)
      this.#timer = setInterval(
        () => void this.#tick(),
        this.#refreshIntervalMs,
      );
  }

  async #tick(): Promise<void> {
    await this.#refreshManagers();
    this.#recomputeAll();
  }

  /** Re-poll every manager to seed/advance clocks and reconcile observations. */
  async #refreshManagers(): Promise<void> {
    await Promise.all(
      [...new Set(this.#managers.values())].map((manager) => manager.refresh()),
    );
  }

  /** Notify `cb` of the current action for each tracked swap, then on every change. */
  subscribeToActions(cb: ActionSubscriber): () => void {
    this.#subscribers.add(cb);
    for (const [swapId, actions] of this.#lastActions) cb(swapId, actions);
    return () => this.#subscribers.delete(cb);
  }

  /** Stop reacting to manager events and drop subscribers. Managers are not owned here. */
  stop(): void {
    if (this.#timer) clearInterval(this.#timer);
    this.#timer = undefined;
    for (const unsub of this.#eventUnsubs) unsub();
    this.#eventUnsubs = [];
    this.#subscribers.clear();
  }

  #managerFor(ref: HtlcRef): ContractManager {
    const manager = this.#managers.get(ref.ledger);
    if (!manager)
      throw new Error(`no ContractManager for ledger '${ref.ledger}'`);
    return manager;
  }

  #recomputeAll(): void {
    for (const swap of this.#swaps.values()) this.#recompute(swap);
  }

  #recompute(swap: TrackedSwap): void {
    // A Lightning swap has one on-chain leg; the absent leg stays `undefined` (its
    // status is derived from the leg that exists). Gate only on present legs — a
    // leg with no observation or clock yet means "not enough known", so bail.
    let clientHtlc: HtlcObservation | undefined;
    let serverHtlc: HtlcObservation | undefined;
    let clientChainNow = 0;
    let serverChainNow = 0;

    if (swap.clientHtlc) {
      const m = this.#managerFor(swap.clientHtlc);
      clientHtlc = m.getState(swap.clientHtlc);
      const now = m.chainNow(swap.clientHtlc);
      if (clientHtlc === undefined || now === undefined) return;
      clientChainNow = now;
    }
    if (swap.serverHtlc) {
      const m = this.#managerFor(swap.serverHtlc);
      serverHtlc = m.getState(swap.serverHtlc);
      const now = m.chainNow(swap.serverHtlc);
      if (serverHtlc === undefined || now === undefined) return;
      serverChainNow = now;
    }

    const status = deriveSwapStatus({ clientHtlc, serverHtlc });
    if (status === undefined) return; // contradictory observations

    const actions = deriveSwapActions({
      status,
      clientChainNow,
      serverChainNow,
      clientRefundLocktime: swap.clientRefundLocktime,
      serverRefundLocktime: swap.serverRefundLocktime,
      // Pay-on-Lightning swaps have no client-funded on-chain leg.
      clientFunds: swap.clientHtlc !== undefined,
    });

    const previous = this.#lastActions.get(swap.swapId);
    if (previous && JSON.stringify(previous) === JSON.stringify(actions))
      return;

    this.#lastActions.set(swap.swapId, actions);
    for (const cb of this.#subscribers) cb(swap.swapId, actions);

    // Terminal: stop watching (retain lastActions so late subscribers still see it).
    if (actions.recommended === "none") this.#untrack(swap);
  }

  #untrack(swap: TrackedSwap): void {
    this.#swaps.delete(swap.swapId);
    for (const leg of legsOf(swap)) void this.#managerFor(leg).unregister(leg);
  }
}

/** The on-chain legs actually present on a swap (one for Lightning, two otherwise). */
function legsOf(swap: TrackedSwap): HtlcRef[] {
  const legs: HtlcRef[] = [];
  if (swap.clientHtlc) legs.push(swap.clientHtlc);
  if (swap.serverHtlc) legs.push(swap.serverHtlc);
  return legs;
}
