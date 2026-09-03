/**
 * A server-trusting stand-in for {@link SwapTracker}: derives each swap's next
 * action from the SERVER's status instead of chain observations — zero chain
 * access (no EVM RPCs, no Esplora/Electrum, no Arkade reads).
 *
 * TEMPORARY default while the chain monitors are being fixed. Unlike
 * {@link SwapTracker} there is no independent verification: the client acts on
 * the server's word. The pushed status is the same `SwapStatus` enum the chain
 * derivation produces, so it feeds straight into {@link deriveSwapActions}.
 * Failure containment: a claim run on a false `serverfunded` fails locally
 * (nothing to spend / gas estimation reverts) rather than losing funds.
 *
 * Timelocks are evaluated against the wall clock, not chain clocks. The wall
 * clock runs AHEAD of Bitcoin's MTP (by up to ~an hour), so a refund can look
 * unlocked slightly early — the broadcast simply fails until the chain agrees,
 * and refunds are user-confirmed anyway — while the claim window closes
 * conservatively early, which is the safe direction.
 */
import type { SwapStatus } from "@lendasat/lendaswap-sdk-pure";
import { deriveSwapActions } from "../actions/derive.js";
import type { SwapActions } from "../actions/types.js";
import type { ActionSubscriber, TrackedSwap } from "../tracker/swap-tracker.js";

export type HintTrackerOptions = {
  /**
   * Fetch a swap's current status from the server — the fallback for hints
   * that don't carry one (the worker's retry re-verifies and its
   * post-reconnect sweep).
   */
  fetchStatus: (swapId: string) => Promise<SwapStatus>;
  /**
   * Local tick interval (ms): recomputes every swap against the wall clock so
   * timelock flips (a refund unlocking, a stale pending swap turning reapable)
   * surface between hints. Pure local work. `0` disables the timer (tests
   * drive recomputes via {@link applyHint}).
   */
  refreshIntervalMs?: number;
};

export class HintTracker {
  readonly #fetchStatus: (swapId: string) => Promise<SwapStatus>;
  readonly #refreshIntervalMs: number;
  readonly #swaps = new Map<string, TrackedSwap>();
  /** Last server-reported status per swap (seeded from storage, updated by hints). */
  readonly #statuses = new Map<string, SwapStatus>();
  /** Last actions emitted per swap — for dedupe and the subscribe-time snapshot. */
  readonly #lastActions = new Map<string, SwapActions>();
  readonly #subscribers = new Set<ActionSubscriber>();
  #timer: ReturnType<typeof setInterval> | undefined;

  constructor(options: HintTrackerOptions) {
    this.#fetchStatus = options.fetchStatus;
    this.#refreshIntervalMs = options.refreshIntervalMs ?? 5_000;
  }

  /**
   * Always `true`: with no chain readers there is no reachability constraint.
   * (Claim/refund execution has its own chain configuration, independent of
   * tracking.)
   */
  canObserve(_swap: TrackedSwap): boolean {
    return true;
  }

  /** Register the swaps, derive each one's first action from its stored status. */
  async startTracking(swaps: TrackedSwap[]): Promise<void> {
    for (const swap of swaps) this.#admit(swap);
    this.#recomputeAll();
    if (this.#refreshIntervalMs > 0)
      this.#timer = setInterval(
        () => this.#recomputeAll(),
        this.#refreshIntervalMs,
      );
  }

  /**
   * Track one more swap after {@link startTracking}. Idempotent: a swap already
   * tracked, or one already seen through to a terminal action, is ignored.
   */
  async track(swap: TrackedSwap): Promise<void> {
    if (this.#swaps.has(swap.swapId) || this.#lastActions.has(swap.swapId))
      return;
    this.#admit(swap);
    this.#recompute(swap);
  }

  #admit(swap: TrackedSwap): void {
    this.#swaps.set(swap.swapId, swap);
    // The stored status may be stale — it only seeds the first derivation; the
    // server's subscribe-time snapshot (pushed right after the worker
    // subscribes) corrects it moments later.
    if (swap.storedStatus !== undefined && !this.#statuses.has(swap.swapId))
      this.#statuses.set(swap.swapId, swap.storedStatus);
  }

  /** The swap ids currently tracked — e.g. to subscribe them to the hint feed. */
  trackedSwapIds(): string[] {
    return [...this.#swaps.keys()];
  }

  /**
   * Apply a server status to one swap and recompute its action. A hint that
   * carries the pushed `status` is applied as-is; without one (the worker's
   * retry and post-reconnect paths) the status is re-fetched from the server.
   * A no-op for an untracked swap.
   */
  async applyHint(
    swapId: string,
    opts?: { force?: boolean; status?: SwapStatus },
  ): Promise<void> {
    const swap = this.#swaps.get(swapId);
    if (!swap) return;
    const status = opts?.status ?? (await this.#fetchStatus(swapId));
    // Re-check: the fetch is async and the swap may have gone terminal (and
    // been untracked) while it was in flight.
    if (!this.#swaps.has(swapId)) return;
    this.#statuses.set(swapId, status);
    this.#recompute(swap, opts?.force ?? false);
  }

  /** Notify `cb` of the current action for each tracked swap, then on every change. */
  subscribeToActions(cb: ActionSubscriber): () => void {
    this.#subscribers.add(cb);
    for (const [swapId, actions] of this.#lastActions) cb(swapId, actions);
    return () => this.#subscribers.delete(cb);
  }

  /** Stop tracking: drop the tick timer, tracked swaps, and subscribers. */
  stop(): void {
    if (this.#timer) clearInterval(this.#timer);
    this.#timer = undefined;
    this.#swaps.clear();
    this.#subscribers.clear();
  }

  #recomputeAll(): void {
    for (const swap of this.#swaps.values()) this.#recompute(swap);
  }

  #recompute(swap: TrackedSwap, force = false): void {
    const status = this.#statuses.get(swap.swapId);
    // No status yet (nothing stored, no hint) — wait for the subscribe-time
    // snapshot rather than deriving from a guess.
    if (status === undefined) return;

    const now = Date.now();
    const actions = deriveSwapActions({
      status,
      clientChainNow: now,
      serverChainNow: now,
      clientRefundLocktime: swap.clientRefundLocktime,
      serverRefundLocktime: swap.serverRefundLocktime,
      // Pay-on-Lightning swaps have no client-funded on-chain leg;
      // receive-on-Lightning swaps have no server-funded one.
      clientFunds: swap.clientHtlc !== undefined,
      serverFunds: swap.serverHtlc !== undefined,
    });
    // No `observations`: nothing was observed on chain. Consumers treat the
    // field as optional.

    const previous = this.#lastActions.get(swap.swapId);
    if (
      !force &&
      previous &&
      JSON.stringify(previous) === JSON.stringify(actions)
    )
      return;

    this.#lastActions.set(swap.swapId, actions);
    for (const cb of this.#subscribers) cb(swap.swapId, actions);

    // Terminal: stop watching (retain lastActions so late subscribers still see it).
    if (actions.recommended === "none") {
      this.#swaps.delete(swap.swapId);
      this.#statuses.delete(swap.swapId);
    }
  }
}
