import { describe, expect, it, vi } from "vitest";
import type { HtlcObservation } from "../actions/types.js";
import {
  type ContractManager,
  type HtlcRef,
  htlcKey,
  type Ledger,
} from "../contracts/types.js";
import { SwapTracker, type TrackedSwap } from "./swap-tracker.js";

/** In-memory ContractManager: push state via `emit`, set the clock via `setNow`. */
class FakeManager implements ContractManager {
  readonly ledger: Ledger;
  #now: number | undefined;
  readonly #states = new Map<string, HtlcObservation>();
  readonly #cbs = new Set<(ref: HtlcRef, state: HtlcObservation) => void>();
  readonly registered = new Set<string>();

  #refreshNow: number | undefined;

  constructor(ledger: Ledger, now?: number, refreshNow?: number) {
    this.ledger = ledger;
    this.#now = now;
    // Mirrors Arkade: the clock is only populated when refresh() runs.
    this.#refreshNow = refreshNow;
  }

  /** Flip to false to simulate an unreachable ledger/chain. */
  observable = true;
  canObserve = (_ref: HtlcRef): boolean => this.observable;
  register = async (ref: HtlcRef): Promise<void> => {
    this.registered.add(htlcKey(ref));
  };
  unregister = async (ref: HtlcRef): Promise<void> => {
    this.registered.delete(htlcKey(ref));
  };
  getState = (ref: HtlcRef): HtlcObservation | undefined =>
    this.#states.get(htlcKey(ref));
  chainNow = (_ref: HtlcRef): number | undefined => this.#now;
  onEvent = (
    cb: (ref: HtlcRef, state: HtlcObservation) => void,
  ): (() => void) => {
    this.#cbs.add(cb);
    return () => this.#cbs.delete(cb);
  };
  refresh = async (): Promise<void> => {
    if (this.#refreshNow !== undefined) this.#now = this.#refreshNow;
  };
  reconciled: HtlcRef[] = [];
  #reconcileState: HtlcObservation | undefined;
  reconcile = async (ref: HtlcRef): Promise<void> => {
    this.reconciled.push(ref);
    if (this.#reconcileState !== undefined)
      this.#states.set(htlcKey(ref), this.#reconcileState);
  };
  dispose = (): void => {};

  /** Make the next reconcile reveal `state` for the ref (as a fresh chain read). */
  setReconcileState(state: HtlcObservation): void {
    this.#reconcileState = state;
  }
  setNow(now: number): void {
    this.#now = now;
  }
  emit(ref: HtlcRef, state: HtlcObservation): void {
    this.#states.set(htlcKey(ref), state);
    for (const cb of this.#cbs) cb(ref, state);
  }
}

const clientHtlc: HtlcRef = {
  ledger: "arkade",
  script: "51ab",
  address: "ark1q",
  preimageHash: "h",
  expectedSats: 0,
  params: {},
};
const serverHtlc: HtlcRef = {
  ledger: "evm",
  chainId: 137,
  htlc: "0xhtlc",
  preimageHash: "0xph",
  claimAddress: "0xc1",
  expectedAmount: 0n,
};
const swap: TrackedSwap = {
  swapId: "s1",
  clientHtlc,
  serverHtlc,
  clientRefundLocktime: 20_000,
  serverRefundLocktime: 10_000,
};

function setup() {
  const arkade = new FakeManager("arkade", 1_000);
  const evm = new FakeManager("evm", 1_000);
  const tracker = new SwapTracker(
    new Map<Ledger, ContractManager>([
      ["arkade", arkade],
      ["evm", evm],
    ]),
  );
  return { arkade, evm, tracker };
}

describe("SwapTracker", () => {
  it("registers both HTLCs on startTracking", async () => {
    const { arkade, evm, tracker } = setup();
    await tracker.startTracking([swap]);
    expect(arkade.registered.has(htlcKey(clientHtlc))).toBe(true);
    expect(evm.registered.has(htlcKey(serverHtlc))).toBe(true);
  });

  it("does not refresh a manager with no registered legs", async () => {
    const arkade = new FakeManager("arkade", 1_000);
    const evm = new FakeManager("evm", 1_000);
    const bitcoin = new FakeManager("bitcoin", 1_000);
    // Bitcoin's clock source is down; the tracked swap has no Bitcoin leg, so its
    // refresh must not run — startTracking should still succeed.
    bitcoin.refresh = async () => {
      throw new Error("MTP endpoint down");
    };
    const tracker = new SwapTracker(
      new Map<Ledger, ContractManager>([
        ["arkade", arkade],
        ["evm", evm],
        ["bitcoin", bitcoin],
      ]),
    );
    await expect(tracker.startTracking([swap])).resolves.toBeUndefined();
  });

  it("starts tracking when one tracked ledger's endpoint is down", async () => {
    // The failing ledger IS part of the swap, so it cannot be skipped the way an
    // untracked one is. Rejecting here used to propagate out of startTracking,
    // which left every swap on every ledger with no derived actions.
    const arkade = new FakeManager("arkade", 1_000);
    const evm = new FakeManager("evm", 1_000);
    evm.refresh = async () => {
      throw new Error("range 44268 exceeds limit of 10000");
    };
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const tracker = new SwapTracker(
      new Map<Ledger, ContractManager>([
        ["arkade", arkade],
        ["evm", evm],
      ]),
    );

    await expect(tracker.startTracking([swap])).resolves.toBeUndefined();
    expect(warn).toHaveBeenCalledWith(
      "SwapTracker: refreshing evm failed:",
      expect.any(Error),
    );

    // Both legs are still registered and the tracker still derives, so the
    // failed refresh cost this round's observations and nothing more.
    expect(arkade.registered.has(htlcKey(clientHtlc))).toBe(true);
    expect(evm.registered.has(htlcKey(serverHtlc))).toBe(true);

    const sub = vi.fn();
    tracker.subscribeToActions(sub);
    arkade.emit(clientHtlc, "confirmed");
    evm.emit(serverHtlc, "confirmed");
    expect(sub).toHaveBeenCalledTimes(1);
    warn.mockRestore();
  });

  it("still applies a hint when one leg cannot be reconciled", async () => {
    // A hint is the fast path. Losing it because one leg was unreadable drops
    // the swap onto the at-risk poller, which re-reads minutes later.
    const arkade = new FakeManager("arkade", 1_000);
    const evm = new FakeManager("evm", 1_000);
    evm.reconcile = async () => {
      throw new Error("range 44636 exceeds limit of 10000");
    };
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const tracker = new SwapTracker(
      new Map<Ledger, ContractManager>([
        ["arkade", arkade],
        ["evm", evm],
      ]),
    );
    await tracker.startTracking([swap]);

    const sub = vi.fn();
    tracker.subscribeToActions(sub);
    sub.mockClear();
    arkade.emit(clientHtlc, "confirmed");
    evm.emit(serverHtlc, "confirmed");

    await expect(tracker.applyHint("s1")).resolves.toBeUndefined();
    expect(warn).toHaveBeenCalledWith(
      "SwapTracker: hint reconcile failed for evm:",
      expect.any(Error),
    );
    expect(sub).toHaveBeenCalled(); // recomputed from what did land
    warn.mockRestore();
  });

  it("primes a refresh-only clock on startTracking, so emits aren't blocked", async () => {
    // Arkade's clock is undefined until refresh(); without priming it in
    // startTracking, the recompute would bail on an undefined clock forever.
    const arkade = new FakeManager("arkade", undefined, 1_000);
    const evm = new FakeManager("evm", 1_000);
    const tracker = new SwapTracker(
      new Map<Ledger, ContractManager>([
        ["arkade", arkade],
        ["evm", evm],
      ]),
    );
    await tracker.startTracking([swap]);
    expect(arkade.chainNow(clientHtlc)).toBe(1_000); // seeded by startTracking

    const sub = vi.fn();
    tracker.subscribeToActions(sub);
    arkade.emit(clientHtlc, "confirmed");
    evm.emit(serverHtlc, "confirmed");
    expect(sub).toHaveBeenCalledTimes(1); // would be 0 if the clock stayed undefined
  });

  it("notifies once both HTLCs are observed (→ serverfunded → claim)", async () => {
    const { arkade, evm, tracker } = setup();
    await tracker.startTracking([swap]);
    const sub = vi.fn();
    tracker.subscribeToActions(sub);

    arkade.emit(clientHtlc, "confirmed"); // only one side known → nothing yet
    expect(sub).not.toHaveBeenCalled();

    evm.emit(serverHtlc, "confirmed"); // both known → serverfunded → claim
    expect(sub).toHaveBeenCalledTimes(1);
    expect(sub).toHaveBeenCalledWith("s1", {
      recommended: "claim",
      actions: [expect.objectContaining({ id: "claim" })],
      // The raw observations ride along for UI progress rendering.
      observations: { clientHtlc: "confirmed", serverHtlc: "confirmed" },
    });
  });

  it("applyHint reconciles the swap's legs, then recomputes from the fresh read", async () => {
    const { arkade, evm, tracker } = setup();
    await tracker.startTracking([swap]);
    const sub = vi.fn();
    tracker.subscribeToActions(sub);

    // A hint arrives; a fresh on-chain read now shows both legs funded.
    arkade.setReconcileState("confirmed");
    evm.setReconcileState("confirmed");
    await tracker.applyHint("s1");

    expect(arkade.reconciled).toContainEqual(clientHtlc);
    expect(evm.reconciled).toContainEqual(serverHtlc);
    expect(sub).toHaveBeenCalledWith(
      "s1",
      expect.objectContaining({ recommended: "claim" }),
    );
  });

  it("track() picks up a swap created after start and derives its action", async () => {
    const { arkade, evm, tracker } = setup();
    await tracker.startTracking([]); // start with nothing tracked
    const sub = vi.fn();
    tracker.subscribeToActions(sub);

    // Both legs are already funded on-chain when the new swap shows up.
    arkade.emit(clientHtlc, "confirmed");
    evm.emit(serverHtlc, "confirmed");
    expect(sub).not.toHaveBeenCalled(); // not tracked yet → nothing derived

    await tracker.track(swap);

    expect(arkade.registered.has(htlcKey(clientHtlc))).toBe(true);
    expect(evm.registered.has(htlcKey(serverHtlc))).toBe(true);
    expect(sub).toHaveBeenCalledWith(
      "s1",
      expect.objectContaining({ recommended: "claim" }),
    );
  });

  it("track() is idempotent for an already-tracked swap", async () => {
    const { arkade, tracker } = setup();
    await tracker.startTracking([swap]);
    arkade.register = vi.fn(arkade.register); // spy further registers
    await tracker.track(swap);
    expect(arkade.register).not.toHaveBeenCalled();
  });

  it("skips a swap with an unreachable leg instead of aborting startTracking", async () => {
    const { arkade, evm, tracker } = setup();
    vi.spyOn(console, "warn").mockImplementation(() => {});
    evm.observable = false; // the EVM chain has no reader configured
    const other: TrackedSwap = { ...swap, swapId: "s2" };

    // s1 (unobservable) is skipped; s2's arkade-only... no, both swaps share legs.
    // Use a swap whose legs are all observable for the "other" case: swap on arkade
    // only would need a different shape, so assert s1 skipped + startTracking ok.
    await expect(tracker.startTracking([swap, other])).resolves.toBeUndefined();
    expect(arkade.registered.size).toBe(0); // neither leg of either swap registered
    expect(tracker.trackedSwapIds()).toEqual([]);
    vi.restoreAllMocks();
  });

  it("canObserve is false when a leg's ledger has no reader", () => {
    const { evm, tracker } = setup();
    expect(tracker.canObserve(swap)).toBe(true);
    evm.observable = false;
    expect(tracker.canObserve(swap)).toBe(false);
  });

  it("track() skips an unreachable swap without throwing", async () => {
    const { arkade, evm, tracker } = setup();
    vi.spyOn(console, "warn").mockImplementation(() => {});
    await tracker.startTracking([]);
    evm.observable = false;

    await expect(tracker.track(swap)).resolves.toBeUndefined();
    expect(arkade.registered.size).toBe(0);
    expect(tracker.trackedSwapIds()).toEqual([]);
    vi.restoreAllMocks();
  });

  it("track() rolls back a partial registration so a later attempt retries", async () => {
    const { arkade, evm, tracker } = setup();
    await tracker.startTracking([]);
    // The EVM leg fails to register, as an RPC/indexer hiccup would. The Arkade
    // leg (registered first) must not be left watched-but-half-tracked.
    const realRegister = evm.register;
    evm.register = async () => {
      throw new Error("rpc down");
    };

    await expect(tracker.track(swap)).rejects.toThrow(/rpc down/);
    expect(arkade.registered.has(htlcKey(clientHtlc))).toBe(false);

    // The swap must not have latched in #swaps — a later sync retries cleanly.
    evm.register = realRegister;
    await tracker.track(swap);
    expect(arkade.registered.has(htlcKey(clientHtlc))).toBe(true);
    expect(evm.registered.has(htlcKey(serverHtlc))).toBe(true);
  });

  it("track() keeps a registered swap when only the initial refresh fails", async () => {
    const { arkade, evm, tracker } = setup();
    await tracker.startTracking([]);
    // Refresh is transient and self-healing (the poll tick retries), so the swap
    // stays registered rather than being rolled back.
    evm.refresh = async () => {
      throw new Error("rpc flaky");
    };

    await expect(tracker.track(swap)).rejects.toThrow(/rpc flaky/);
    expect(arkade.registered.has(htlcKey(clientHtlc))).toBe(true);
    expect(evm.registered.has(htlcKey(serverHtlc))).toBe(true);
  });

  it("track() ignores a swap already seen through to terminal", async () => {
    const { arkade, evm, tracker } = setup();
    await tracker.startTracking([swap]);
    tracker.subscribeToActions(vi.fn());
    arkade.emit(clientHtlc, "confirmed");
    evm.emit(serverHtlc, "confirmed"); // claim
    evm.emit(serverHtlc, "spent_claim"); // terminal none → untracked
    expect(arkade.registered.has(htlcKey(clientHtlc))).toBe(false);

    await tracker.track(swap); // rediscovered — must not resurrect it
    expect(arkade.registered.has(htlcKey(clientHtlc))).toBe(false);
  });

  it("applyHint is a no-op for an untracked swap", async () => {
    const { arkade, evm, tracker } = setup();
    await tracker.startTracking([swap]);
    await tracker.applyHint("unknown");
    expect(arkade.reconciled).toEqual([]);
    expect(evm.reconciled).toEqual([]);
  });

  it("dedupes: an unchanged observation does not re-notify", async () => {
    const { arkade, evm, tracker } = setup();
    await tracker.startTracking([swap]);
    const sub = vi.fn();
    tracker.subscribeToActions(sub);

    arkade.emit(clientHtlc, "confirmed");
    evm.emit(serverHtlc, "confirmed");
    expect(sub).toHaveBeenCalledTimes(1);

    arkade.emit(clientHtlc, "confirmed"); // same state → no new notification
    expect(sub).toHaveBeenCalledTimes(1);
  });

  it("reacts to a state change and drops the swap once terminal", async () => {
    const { arkade, evm, tracker } = setup();
    await tracker.startTracking([swap]);
    const sub = vi.fn();
    tracker.subscribeToActions(sub);

    arkade.emit(clientHtlc, "confirmed");
    evm.emit(serverHtlc, "confirmed"); // serverfunded → claim
    evm.emit(serverHtlc, "spent_claim"); // client redeemed → terminal none

    expect(sub).toHaveBeenCalledTimes(2);
    expect(sub).toHaveBeenLastCalledWith("s1", {
      recommended: "none",
      actions: [expect.objectContaining({ id: "none" })],
      observations: { clientHtlc: "confirmed", serverHtlc: "spent_claim" },
    });
    // terminal → unregistered, and further events don't notify
    expect(evm.registered.has(htlcKey(serverHtlc))).toBe(false);
    arkade.emit(clientHtlc, "spent_claim");
    expect(sub).toHaveBeenCalledTimes(2);
  });

  it("gives a late subscriber the current snapshot immediately", async () => {
    const { arkade, evm, tracker } = setup();
    await tracker.startTracking([swap]);
    arkade.emit(clientHtlc, "confirmed");
    evm.emit(serverHtlc, "confirmed");

    const late = vi.fn();
    tracker.subscribeToActions(late);
    expect(late).toHaveBeenCalledTimes(1);
    expect(late).toHaveBeenCalledWith(
      "s1",
      expect.objectContaining({ recommended: "claim" }),
    );
  });

  it("waits for the chain clock before emitting", async () => {
    const arkade = new FakeManager("arkade"); // no clock yet
    const evm = new FakeManager("evm", 1_000);
    const tracker = new SwapTracker(
      new Map<Ledger, ContractManager>([
        ["arkade", arkade],
        ["evm", evm],
      ]),
    );
    await tracker.startTracking([swap]);
    const sub = vi.fn();
    tracker.subscribeToActions(sub);

    arkade.emit(clientHtlc, "confirmed");
    evm.emit(serverHtlc, "confirmed");
    expect(sub).not.toHaveBeenCalled(); // arkade clock unknown → no action yet

    arkade.setNow(1_000);
    arkade.emit(clientHtlc, "confirmed"); // same state, but now the clock is known
    expect(sub).toHaveBeenCalledTimes(1);
  });

  describe("at-risk chain reconciles (fake time)", () => {
    const buildTimed = () => {
      vi.useFakeTimers();
      const arkade = new FakeManager("arkade", 1_000);
      const evm = new FakeManager("evm", 1_000);
      const tracker = new SwapTracker(
        new Map<Ledger, ContractManager>([
          ["arkade", arkade],
          ["evm", evm],
        ]),
        { refreshIntervalMs: 1_000, atRiskReconcileIntervalMs: 60_000 },
      );
      return { arkade, evm, tracker };
    };

    it("re-reads the chain for a swap whose client leg holds funds", async () => {
      const { arkade, evm, tracker } = buildTimed();
      try {
        await tracker.startTracking([swap]);
        arkade.emit(clientHtlc, "confirmed"); // client funds on-chain → at risk
        evm.emit(serverHtlc, "absent");

        await vi.advanceTimersByTimeAsync(59_000);
        expect(arkade.reconciled).toHaveLength(0); // within the interval: free

        await vi.advanceTimersByTimeAsync(2_000);
        // Both legs re-read: refund availability AND the counterparty's state.
        expect(arkade.reconciled.map(htlcKey)).toEqual([htlcKey(clientHtlc)]);
        expect(evm.reconciled.map(htlcKey)).toEqual([htlcKey(serverHtlc)]);
      } finally {
        tracker.stop();
        vi.useRealTimers();
      }
    });

    it("retries a swap whose startup registration failed", async () => {
      const { arkade, evm, tracker } = buildTimed();
      try {
        // One transient failure on the EVM leg's initial chain read.
        let attempts = 0;
        const register = evm.register;
        evm.register = async (ref) => {
          if (attempts++ === 0) throw new Error("rpc down");
          return register(ref);
        };

        await tracker.startTracking([swap]);
        // Skipped, not aborted: tracking is up, the swap is parked, and its
        // Arkade leg was rolled back rather than left half-watched.
        expect(tracker.trackedSwapIds()).toEqual([]);
        expect(arkade.registered.size).toBe(0);

        await vi.advanceTimersByTimeAsync(61_000);
        expect(tracker.trackedSwapIds()).toEqual([swap.swapId]);
        expect(arkade.registered.size).toBe(1);
        expect(evm.registered.size).toBe(1);
      } finally {
        tracker.stop();
        vi.useRealTimers();
      }
    });

    it("costs zero chain reads while the client leg is unfunded (hints only)", async () => {
      const { arkade, evm, tracker } = buildTimed();
      try {
        await tracker.startTracking([swap]);
        arkade.emit(clientHtlc, "absent"); // nothing at stake yet
        evm.emit(serverHtlc, "absent");

        await vi.advanceTimersByTimeAsync(180_000);
        expect(arkade.reconciled).toHaveLength(0);
        expect(evm.reconciled).toHaveLength(0);
      } finally {
        tracker.stop();
        vi.useRealTimers();
      }
    });

    it("treats a swap with no on-chain client leg (Lightning pay-in) as at risk", async () => {
      const { evm, tracker } = buildTimed();
      try {
        const lnSwap: TrackedSwap = {
          swapId: "ln1",
          serverHtlc,
          clientRefundLocktime: 20_000,
          serverRefundLocktime: 10_000,
        };
        await tracker.startTracking([lnSwap]);
        evm.emit(serverHtlc, "absent");

        await vi.advanceTimersByTimeAsync(61_000);
        expect(evm.reconciled.map(htlcKey)).toEqual([htlcKey(serverHtlc)]);
      } finally {
        tracker.stop();
        vi.useRealTimers();
      }
    });
  });
});
