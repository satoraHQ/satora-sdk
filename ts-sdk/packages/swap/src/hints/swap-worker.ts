/**
 * Ties the hint feed and the action model together into auto-execution.
 *
 * On a server status hint it verifies the swap against the chain
 * (`tracker.applyHint`) — the hint only makes the derivation faster, the chain
 * stays the source of truth. Then, on every derived action, it AUTO-RUNS the
 * recommended action when it is auto-safe (a claim) and SURFACES the rest — a
 * manual `fund`, or a refund the user must confirm — via `onActionRequired`, the
 * seam a frontend notification center plugs into.
 *
 * A failed auto-claim is RETRIED with backoff. Each retry first re-verifies
 * against chain (a forced `applyHint`) and only re-runs if the swap is still
 * claimable — so we never blindly re-submit a claim (which on EVM would waste gas
 * or double-submit). After the attempt budget is spent the swap is surfaced via
 * `onActionRequired`, so it is never silently stuck.
 */
import type { SwapStatus } from "@lendasat/lendaswap-sdk-pure";
import type { SwapActionId, SwapActions } from "../actions/types.js";

/** The tracker surface the worker drives. */
export interface WorkerTracker {
  trackedSwapIds(): string[];
  /**
   * Re-verify a swap, then recompute. `force` re-emits even if unchanged.
   * `status` is the hint's pushed status: a chain-verifying tracker ignores it
   * (the chain stays its source of truth); a hints-only tracker applies it
   * directly, saving a server round-trip.
   */
  applyHint(
    swapId: string,
    opts?: { force?: boolean; status?: SwapStatus },
  ): Promise<void>;
  subscribeToActions(
    cb: (swapId: string, actions: SwapActions) => void,
  ): () => void;
}

/** The hint feed surface the worker consumes (`WsStatusSource` satisfies it). */
export interface WorkerHintSource {
  start(): void;
  stop(): void;
  subscribe(swapIds: string[]): void;
  unsubscribe(swapIds: string[]): void;
  onStatus(
    cb: (update: { swapId: string; status?: SwapStatus }) => void,
  ): () => void;
  /**
   * Optional: fires when the feed comes back up after an outage, during which
   * pushed transitions were lost for good. The worker answers by re-verifying
   * every tracked swap against the chain.
   */
  onReconnect?(cb: () => void): () => void;
}

/** Backoff policy for retrying a failed auto-claim. */
export type SwapWorkerRetryOptions = {
  /** Attempts before giving up and surfacing the swap. Default 5. */
  maxAttempts?: number;
  /** Base backoff (ms), doubled each attempt. Default 2000. */
  baseDelayMs?: number;
  /** Backoff ceiling (ms). Default 30000. */
  maxDelayMs?: number;
};

export type SwapWorkerOptions = {
  tracker: WorkerTracker;
  hintSource: WorkerHintSource;
  /**
   * Run the recommended auto action for a swap (today: claim). Reject to signal
   * failure — the worker re-verifies against chain and retries with backoff.
   * Resolve once submitted. OMIT for observe-only mode: hints still trigger
   * chain re-verifies (fast, cheap derivations), but nothing is ever run.
   */
  execute?: (swapId: string, actionId: SwapActionId) => Promise<void>;
  /**
   * Surface an action that needs the user rather than being auto-run — a manual
   * `fund`, a refund the user must confirm, or a claim whose retries are spent.
   * Where a UI notification hooks in.
   */
  onActionRequired?: (swapId: string, actions: SwapActions) => void;
  /** Auto-claim retry policy; sensible defaults otherwise. */
  retry?: SwapWorkerRetryOptions;
};

/** Actions the worker runs automatically. `wait`/`none` are auto but no-ops. */
const AUTO_EXECUTABLE: ReadonlySet<SwapActionId> = new Set<SwapActionId>([
  "claim",
]);

/** In-flight retry bookkeeping for one swap. */
type Retry = { attempts: number; timer: ReturnType<typeof setTimeout> };

export class SwapWorker {
  readonly #tracker: WorkerTracker;
  readonly #hintSource: WorkerHintSource;
  readonly #execute: SwapWorkerOptions["execute"];
  readonly #onActionRequired: SwapWorkerOptions["onActionRequired"];
  readonly #maxAttempts: number;
  readonly #baseDelayMs: number;
  readonly #maxDelayMs: number;

  /** Swaps with an auto action in flight — guards against double-runs. */
  readonly #executing = new Set<string>();
  /** Swaps with a scheduled auto-claim retry (backoff timer + attempt count). */
  readonly #retries = new Map<string, Retry>();
  /**
   * Swaps whose claim needs the user's wallet (RIF Relay): handed to
   * `onActionRequired` once and not auto-run while the claim is recommended.
   */
  readonly #needsUser = new Set<string>();
  #unsubs: Array<() => void> = [];
  #started = false;

  constructor(options: SwapWorkerOptions) {
    this.#tracker = options.tracker;
    this.#hintSource = options.hintSource;
    this.#execute = options.execute;
    this.#onActionRequired = options.onActionRequired;
    this.#maxAttempts = options.retry?.maxAttempts ?? 5;
    this.#baseDelayMs = options.retry?.baseDelayMs ?? 2_000;
    this.#maxDelayMs = options.retry?.maxDelayMs ?? 30_000;
  }

  /** Watch tracked swaps on the hint feed, wire hints → verify, react to actions. */
  start(): void {
    if (this.#started) return;
    this.#started = true;

    this.#hintSource.subscribe(this.#tracker.trackedSwapIds());
    this.#unsubs.push(
      this.#hintSource.onStatus(({ swapId, status }) => {
        void this.#tracker.applyHint(swapId, { status }).catch((error) => {
          console.warn(`SwapWorker: applyHint(${swapId}) failed:`, error);
        });
      }),
    );
    // Hints were dead while the feed was down and pushed transitions are lost
    // for good — anything may have happened, including funding an `absent`
    // client leg, the one state nothing else re-reads (it is outside the
    // tracker's at-risk cadence). The server's resubscribe snapshot usually
    // re-triggers applyHint for subscribed ids, but this sweep is the
    // client-side guarantee: it doesn't depend on the server sending one, and
    // it also covers over-cap queued ids that never get hints.
    if (this.#hintSource.onReconnect)
      this.#unsubs.push(
        this.#hintSource.onReconnect(() => this.#reverifyAllTracked()),
      );
    this.#hintSource.start();

    this.#unsubs.push(
      this.#tracker.subscribeToActions((swapId, actions) =>
        this.#onActions(swapId, actions),
      ),
    );
  }

  /** Stop reacting, cancel pending retries, and close the hint feed. */
  stop(): void {
    this.#started = false;
    for (const unsub of this.#unsubs) unsub();
    this.#unsubs = [];
    this.#hintSource.stop();
    for (const { timer } of this.#retries.values()) clearTimeout(timer);
    this.#retries.clear();
    this.#needsUser.clear();
    this.#executing.clear();
  }

  /** Chain-reconcile every tracked swap (the post-outage catch-up sweep). */
  #reverifyAllTracked(): void {
    for (const swapId of this.#tracker.trackedSwapIds()) {
      void this.#tracker.applyHint(swapId).catch((error) => {
        console.warn(
          `SwapWorker: post-reconnect re-verify of ${swapId} failed:`,
          error,
        );
      });
    }
  }

  #onActions(swapId: string, actions: SwapActions): void {
    const recommended = actions.recommended;
    // Terminal first: a finished swap needs no hints, and subscribing before
    // dropping it would briefly consume a subscription slot (or queue behind the
    // server cap) on a swap that is already done.
    if (recommended === "none") {
      this.#hintSource.unsubscribe([swapId]);
      this.#cancelRetry(swapId);
      return;
    }
    // Ensure the swap is on the hint feed (covers swaps whose first action only
    // appears after tracking started).
    this.#hintSource.subscribe([swapId]);

    // Observe-only mode (no execute): fall through — an auto action is nobody's
    // to run, and it doesn't need surfacing either (subscribers see it live).
    if (recommended && AUTO_EXECUTABLE.has(recommended) && this.#execute) {
      if (this.#needsUser.has(swapId)) return; // already surfaced
      this.#autoExecute(swapId, actions);
      return;
    }
    this.#needsUser.delete(swapId);
    // No longer an auto-claim (moved to wait/fund/refund, or nothing): drop any
    // pending retry — a fresh derivation supersedes it.
    this.#cancelRetry(swapId);
    // Surface only actions that actually need the user — a `manual` fund or a
    // refund to `confirm`. `wait` is an auto no-op: nothing to notify about.
    if (!recommended) return;
    const automation = actions.actions.find(
      (a) => a.id === recommended,
    )?.automation;
    if (automation === "manual" || automation === "confirm")
      this.#onActionRequired?.(swapId, actions);
  }

  #autoExecute(swapId: string, actions: SwapActions): void {
    const execute = this.#execute;
    if (!execute) return; // observe-only mode
    if (this.#executing.has(swapId)) return; // one in-flight run per swap
    const actionId = actions.recommended;
    if (!actionId) return;
    this.#executing.add(swapId);
    void execute(swapId, actionId)
      .then(() => this.#cancelRetry(swapId)) // submitted — reset the attempt budget
      .catch((error) => {
        if (needsWallet(error)) {
          // No retry can sign for the user's wallet: hand it over now.
          this.#cancelRetry(swapId);
          this.#needsUser.add(swapId);
          this.#onActionRequired?.(swapId, actions);
          return;
        }
        console.warn(`SwapWorker: ${actionId}(${swapId}) failed:`, error);
        this.#scheduleRetry(swapId, actions);
      })
      .finally(() => this.#executing.delete(swapId));
  }

  /** Schedule the next backoff retry, or give up and surface once attempts run out. */
  #scheduleRetry(swapId: string, actions: SwapActions): void {
    const attempts = (this.#retries.get(swapId)?.attempts ?? 0) + 1;
    if (attempts > this.#maxAttempts) {
      // Out of attempts: stop auto-running and hand it to the user (a UI can offer
      // a manual claim). Better than silently stuck.
      console.warn(
        `SwapWorker: auto-claim for ${swapId} gave up after ${this.#maxAttempts} attempts`,
      );
      this.#cancelRetry(swapId);
      this.#onActionRequired?.(swapId, actions);
      return;
    }
    this.#clearTimer(swapId);
    const delay = Math.min(
      this.#baseDelayMs * 2 ** (attempts - 1),
      this.#maxDelayMs,
    );
    const timer = setTimeout(() => this.#retry(swapId, actions), delay);
    this.#retries.set(swapId, { attempts, timer });
  }

  /**
   * A retry fires: re-verify against chain (forced re-emit), which drives
   * {@link #onActions} to re-run the claim only if it is still recommended. If the
   * re-verify itself fails we can't safely retry, so back off and try the cycle
   * again.
   */
  #retry(swapId: string, actions: SwapActions): void {
    void this.#tracker.applyHint(swapId, { force: true }).catch((error) => {
      console.warn(
        `SwapWorker: re-verify before retrying ${swapId} failed:`,
        error,
      );
      this.#scheduleRetry(swapId, actions);
    });
  }

  #cancelRetry(swapId: string): void {
    this.#clearTimer(swapId);
    this.#retries.delete(swapId);
  }

  #clearTimer(swapId: string): void {
    const existing = this.#retries.get(swapId);
    if (existing) clearTimeout(existing.timer);
  }
}

/**
 * The claim needs the receiving wallet's signature (the SDK's
 * `WalletClaimRequiredError`); matched by name so it holds across bundles.
 */
function needsWallet(error: unknown): boolean {
  return error instanceof Error && error.name === "WalletClaimRequiredError";
}
