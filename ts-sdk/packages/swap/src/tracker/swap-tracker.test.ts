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
  dispose = (): void => {};

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
    });
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
});
