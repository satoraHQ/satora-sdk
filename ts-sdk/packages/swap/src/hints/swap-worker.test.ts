import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  SwapActionAutomation,
  SwapActionId,
  SwapActions,
} from "../actions/types.js";
import {
  SwapWorker,
  type WorkerHintSource,
  type WorkerTracker,
} from "./swap-worker.js";

class FakeTracker implements WorkerTracker {
  ids: string[] = [];
  applied: string[] = [];
  #cb: ((id: string, a: SwapActions) => void) | undefined;
  readonly #last = new Map<string, SwapActions>();
  trackedSwapIds(): string[] {
    return this.ids;
  }
  applyHint = async (id: string, opts?: { force?: boolean }): Promise<void> => {
    this.applied.push(id);
    // A forced re-verify re-emits the swap's current (unchanged) action, standing
    // in for "chain still says claimable" so a retry re-runs.
    const current = this.#last.get(id);
    if (opts?.force && current) this.#cb?.(id, current);
  };
  subscribeToActions(cb: (id: string, a: SwapActions) => void): () => void {
    this.#cb = cb;
    return () => {
      this.#cb = undefined;
    };
  }
  emit(id: string, actions: SwapActions): void {
    this.#last.set(id, actions);
    this.#cb?.(id, actions);
  }
}

class FakeHintSource implements WorkerHintSource {
  started = false;
  stopped = false;
  subscribed: string[] = [];
  unsubscribed: string[] = [];
  #cb: ((u: { swapId: string }) => void) | undefined;
  start(): void {
    this.started = true;
  }
  stop(): void {
    this.stopped = true;
  }
  subscribe(ids: string[]): void {
    this.subscribed.push(...ids);
  }
  unsubscribe(ids: string[]): void {
    this.unsubscribed.push(...ids);
  }
  onStatus(cb: (u: { swapId: string }) => void): () => void {
    this.#cb = cb;
    return () => {
      this.#cb = undefined;
    };
  }
  onReconnect(cb: () => void): () => void {
    this.#reconnectCb = cb;
    return () => {
      this.#reconnectCb = undefined;
    };
  }
  #reconnectCb: (() => void) | undefined;
  hint(swapId: string): void {
    this.#cb?.({ swapId });
  }
  reconnect(): void {
    this.#reconnectCb?.();
  }
}

/** Automation per action id — mirrors what `deriveSwapActions` assigns. */
const AUTOMATION: Record<SwapActionId, SwapActionAutomation> = {
  wait: "auto",
  fund: "manual",
  claim: "auto",
  refund_collaborative: "confirm",
  refund_unilateral: "confirm",
  recover_cctp_claim: "confirm",
  none: "auto",
};

/** A `SwapActions` whose recommended action carries its real automation. */
const acts = (recommended?: SwapActionId): SwapActions =>
  (recommended === undefined
    ? { recommended, actions: [] }
    : {
        recommended,
        actions: [
          {
            id: recommended,
            recommended: true,
            automation: AUTOMATION[recommended],
            reason: "",
          },
        ],
      }) as SwapActions;

function setup(
  execute = vi.fn(async () => {}),
  retry?: { maxAttempts?: number; baseDelayMs?: number; maxDelayMs?: number },
) {
  const tracker = new FakeTracker();
  const hintSource = new FakeHintSource();
  const onActionRequired = vi.fn();
  const worker = new SwapWorker({
    tracker,
    hintSource,
    execute,
    onActionRequired,
    retry,
  });
  return { tracker, hintSource, worker, execute, onActionRequired };
}

describe("SwapWorker", () => {
  it("subscribes the tracked swaps to the hint feed and starts it", () => {
    const { tracker, hintSource, worker } = setup();
    tracker.ids = ["a", "b"];
    worker.start();
    expect(hintSource.subscribed).toEqual(["a", "b"]);
    expect(hintSource.started).toBe(true);
  });

  it("verifies against the chain on a hint (applyHint)", () => {
    const { tracker, hintSource, worker } = setup();
    worker.start();
    hintSource.hint("s1");
    expect(tracker.applied).toEqual(["s1"]);
  });

  it("auto-executes a recommended claim", async () => {
    const { tracker, worker, execute } = setup();
    worker.start();
    tracker.emit("s1", acts("claim"));
    expect(execute).toHaveBeenCalledWith("s1", "claim");
  });

  it("hands a claim that needs the user's wallet over once, without retrying", async () => {
    const walletRequired = Object.assign(new Error("needs wallet"), {
      name: "WalletClaimRequiredError",
    });
    const execute = vi.fn(async () => {
      throw walletRequired;
    });
    const { tracker, worker, onActionRequired } = setup(execute, {
      baseDelayMs: 1,
    });
    worker.start();
    const claim = acts("claim");
    tracker.emit("s1", claim);
    await vi.waitFor(() => expect(onActionRequired).toHaveBeenCalledTimes(1));
    expect(onActionRequired).toHaveBeenCalledWith("s1", claim);

    // A re-emit of the same claim is not run again.
    tracker.emit("s1", claim);
    await new Promise((r) => setTimeout(r, 10));
    expect(execute).toHaveBeenCalledTimes(1);
    expect(onActionRequired).toHaveBeenCalledTimes(1);

    // Once the claim stops being recommended, a later one is tried again.
    tracker.emit("s1", acts("wait"));
    tracker.emit("s1", claim);
    expect(execute).toHaveBeenCalledTimes(2);
  });

  it("observe-only mode (no execute): hints verify, but a claim is never run", () => {
    const tracker = new FakeTracker();
    const hintSource = new FakeHintSource();
    const onActionRequired = vi.fn();
    const worker = new SwapWorker({ tracker, hintSource, onActionRequired });
    worker.start();

    hintSource.hint("s1");
    expect(tracker.applied).toEqual(["s1"]); // hint → chain verify still happens

    tracker.emit("s1", acts("claim"));
    // Nothing to run and nothing to surface (claim is auto; subscribers see it).
    expect(onActionRequired).not.toHaveBeenCalled();
    // Still surfaces what genuinely needs the user.
    tracker.emit("s2", acts("refund_unilateral"));
    expect(onActionRequired).toHaveBeenCalledWith(
      "s2",
      expect.objectContaining({ recommended: "refund_unilateral" }),
    );
  });

  it("re-verifies every tracked swap against chain when the hint feed reconnects", () => {
    const { tracker, hintSource, worker } = setup();
    tracker.ids = ["s1", "s2"];
    worker.start();

    // An outage loses pushed transitions for good — on reconnect the worker
    // must not wait for hints (a funded-while-down `absent` leg never gets one).
    hintSource.reconnect();
    expect(tracker.applied).toEqual(["s1", "s2"]);
  });

  it("surfaces a refund instead of running it", () => {
    const { tracker, worker, execute, onActionRequired } = setup();
    worker.start();
    const refund = acts("refund_unilateral");
    tracker.emit("s1", refund);
    expect(execute).not.toHaveBeenCalled();
    expect(onActionRequired).toHaveBeenCalledWith("s1", refund);
  });

  it("surfaces a manual fund instead of running it", () => {
    const { tracker, worker, execute, onActionRequired } = setup();
    worker.start();
    tracker.emit("s1", acts("fund"));
    expect(execute).not.toHaveBeenCalled();
    expect(onActionRequired).toHaveBeenCalledWith("s1", acts("fund"));
  });

  it("does not surface 'wait' — an auto no-op needs no user action", () => {
    const { tracker, worker, execute, onActionRequired } = setup();
    worker.start();
    tracker.emit("s1", acts("wait"));
    expect(execute).not.toHaveBeenCalled();
    expect(onActionRequired).not.toHaveBeenCalled();
  });

  it("does not double-run a claim while one is in flight", () => {
    let release!: () => void;
    const execute = vi.fn(() => new Promise<void>((r) => (release = r)));
    const { tracker, worker } = setup(execute);
    worker.start();
    tracker.emit("s1", acts("claim"));
    tracker.emit("s1", acts("claim")); // still in flight
    expect(execute).toHaveBeenCalledTimes(1);
    release();
  });

  it("unsubscribes a swap from the hint feed on a terminal action", () => {
    const { tracker, hintSource, worker } = setup();
    worker.start();
    tracker.emit("s1", acts("none"));
    expect(hintSource.unsubscribed).toContain("s1");
  });

  it("stop() detaches and closes the hint feed", () => {
    const { tracker, hintSource, worker, execute } = setup();
    worker.start();
    worker.stop();
    expect(hintSource.stopped).toBe(true);
    // No longer reacting after stop.
    tracker.emit("s1", acts("claim"));
    hintSource.hint("s1");
    expect(execute).not.toHaveBeenCalled();
    expect(tracker.applied).toEqual([]);
  });

  describe("auto-claim retry", () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    const fast = { maxAttempts: 5, baseDelayMs: 10, maxDelayMs: 10 };

    it("re-verifies against chain then re-runs a failed claim, stopping on success", async () => {
      let n = 0;
      const execute = vi.fn(async () => {
        n += 1;
        if (n === 1) throw new Error("rpc blip"); // first attempt fails
      });
      const { tracker, worker } = setup(execute, fast);
      worker.start();

      tracker.emit("s1", acts("claim"));
      await vi.advanceTimersByTimeAsync(0); // settle the failure → schedule retry
      expect(execute).toHaveBeenCalledTimes(1);
      expect(tracker.applied).toEqual([]); // not re-verified yet

      await vi.advanceTimersByTimeAsync(15); // retry fires → applyHint(force) → re-run
      expect(tracker.applied).toContain("s1"); // re-verified before retrying
      expect(execute).toHaveBeenCalledTimes(2); // second attempt (succeeds)

      // Succeeded → no further retries scheduled.
      await vi.advanceTimersByTimeAsync(100);
      expect(execute).toHaveBeenCalledTimes(2);
    });

    it("gives up and surfaces the swap after exhausting attempts", async () => {
      const execute = vi.fn(async () => {
        throw new Error("always fails");
      });
      const { tracker, worker, onActionRequired } = setup(execute, {
        ...fast,
        maxAttempts: 3,
      });
      worker.start();

      tracker.emit("s1", acts("claim"));
      // Drain the initial attempt + 3 retries.
      for (let i = 0; i < 5; i++) await vi.advanceTimersByTimeAsync(15);

      expect(execute).toHaveBeenCalledTimes(4); // initial + 3 retries
      expect(onActionRequired).toHaveBeenCalledWith("s1", acts("claim"));
      // No more retries after giving up.
      const after = execute.mock.calls.length;
      await vi.advanceTimersByTimeAsync(100);
      expect(execute).toHaveBeenCalledTimes(after);
    });

    it("cancels a pending retry when the swap moves off claim", async () => {
      const execute = vi.fn(async () => {
        throw new Error("blip");
      });
      const { tracker, worker } = setup(execute, {
        maxAttempts: 5,
        baseDelayMs: 50,
        maxDelayMs: 50,
      });
      worker.start();

      tracker.emit("s1", acts("claim"));
      await vi.advanceTimersByTimeAsync(0); // 1st attempt fails → retry scheduled
      expect(execute).toHaveBeenCalledTimes(1);

      tracker.emit("s1", acts("wait")); // state moved on → supersedes the retry
      await vi.advanceTimersByTimeAsync(100); // the 50ms timer would have fired
      expect(execute).toHaveBeenCalledTimes(1); // but was cancelled
    });

    it("stop() cancels pending retries", async () => {
      const execute = vi.fn(async () => {
        throw new Error("blip");
      });
      const { tracker, worker } = setup(execute, {
        maxAttempts: 5,
        baseDelayMs: 50,
        maxDelayMs: 50,
      });
      worker.start();

      tracker.emit("s1", acts("claim"));
      await vi.advanceTimersByTimeAsync(0);
      worker.stop();

      await vi.advanceTimersByTimeAsync(100);
      expect(execute).toHaveBeenCalledTimes(1); // no retry after stop
    });
  });
});
