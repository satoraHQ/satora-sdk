import type { SwapStatus } from "@lendasat/lendaswap-sdk-pure";
import { describe, expect, it } from "vitest";
import { deriveSwapActions } from "./derive.js";
import type { SwapActionInput } from "./types.js";

/**
 * Every lifecycle status. `satisfies` rejects a typo'd value; the exhaustiveness
 * check below rejects a *missing* one — so adding a `SwapStatus` breaks this test
 * until it's listed (and considered by the guard tests).
 */
const ALL_STATUSES = [
  "pending",
  "clientfundingseen",
  "clientfunded",
  "clientrefunded",
  "serverfunded",
  "clientredeeming",
  "clientredeemed",
  "serverredeemed",
  "clientfundedserverrefunded",
  "clientrefundedserverfunded",
  "clientrefundedserverrefunded",
  "expired",
  "clientinvalidfunded",
  "clientfundedtoolate",
  "serverwontfund",
  "clientredeemedandclientrefunded",
] as const satisfies readonly SwapStatus[];

// Compile-time exhaustiveness: fails to build if a SwapStatus is not listed.
type _MissingStatus = Exclude<SwapStatus, (typeof ALL_STATUSES)[number]>;
const _exhaustive: _MissingStatus extends never ? true : false = true;
void _exhaustive;

/**
 * Snapshot with both chain clocks at 1_000 and both locktimes at 10_000 (so
 * nothing is expired by default); override only what a test cares about.
 */
function input(
  overrides: Partial<SwapActionInput> & Pick<SwapActionInput, "status">,
): SwapActionInput {
  return {
    clientChainNow: 1_000,
    serverChainNow: 1_000,
    clientRefundLocktime: 10_000,
    serverRefundLocktime: 10_000,
    ...overrides,
  };
}

describe("deriveSwapActions", () => {
  describe("pending", () => {
    it("before the funding deadline → one recommended, manual fund action", () => {
      expect(deriveSwapActions(input({ status: "pending" }))).toEqual({
        recommended: "fund",
        actions: [
          {
            id: "fund",
            recommended: true,
            automation: "manual",
            reason: expect.any(String),
          },
        ],
      });
    });

    it("at the client refund locktime → no longer fundable (client-chain clock)", () => {
      const result = deriveSwapActions(
        input({ status: "pending", clientChainNow: 10_000 }),
      );
      expect(result.actions).toEqual([]);
      expect(result.recommended).toBeUndefined();
    });

    it("past the locktime → no fund action", () => {
      expect(
        deriveSwapActions(input({ status: "pending", clientChainNow: 20_000 }))
          .actions,
      ).toEqual([]);
    });
  });

  describe("clientfundingseen", () => {
    it("→ wait for the funding tx to confirm", () => {
      expect(deriveSwapActions(input({ status: "clientfundingseen" }))).toEqual(
        {
          recommended: "wait",
          actions: [
            {
              id: "wait",
              recommended: true,
              automation: "auto",
              reason: expect.any(String),
            },
          ],
        },
      );
    });
  });

  describe("clientfunded", () => {
    it("within the timelock → wait recommended, refund surfaced but blocked", () => {
      expect(deriveSwapActions(input({ status: "clientfunded" }))).toEqual({
        recommended: "wait",
        actions: [
          {
            id: "wait",
            recommended: true,
            automation: "auto",
            reason: expect.any(String),
          },
          {
            id: "refund_unilateral",
            recommended: false,
            automation: "confirm",
            reason: expect.any(String),
            blockedBy: {
              kind: "timelock_not_expired",
              message: expect.any(String),
            },
          },
        ],
      });
    });

    it("at the timelock → refund becomes runnable and recommended", () => {
      expect(
        deriveSwapActions(
          input({ status: "clientfunded", clientChainNow: 10_000 }),
        ),
      ).toEqual({
        recommended: "refund_unilateral",
        actions: [
          {
            id: "refund_unilateral",
            recommended: true,
            automation: "confirm",
            reason: expect.any(String),
          },
        ],
      });
    });

    it("past the timelock → refund runnable (no blockedBy)", () => {
      const refund = deriveSwapActions(
        input({ status: "clientfunded", clientChainNow: 20_000 }),
      ).actions[0];
      expect(refund?.id).toBe("refund_unilateral");
      expect(refund?.blockedBy).toBeUndefined();
    });
  });

  describe("serverfunded", () => {
    // Server's refund locktime (10_000) is earlier than the client's (20_000),
    // as in a real swap — so the claim window closes before the client can refund.
    const base = {
      status: "serverfunded" as const,
      clientRefundLocktime: 20_000,
    };

    it("within the claim window → claim recommended and auto-runnable", () => {
      expect(
        deriveSwapActions(input({ ...base, serverChainNow: 1_000 })),
      ).toEqual({
        recommended: "claim",
        actions: [
          {
            id: "claim",
            recommended: true,
            automation: "auto",
            reason: expect.any(String),
          },
        ],
      });
    });

    it("claim window closed, refund not yet unlocked → wait, never claim", () => {
      const result = deriveSwapActions(
        input({ ...base, serverChainNow: 15_000, clientChainNow: 1_000 }),
      );
      expect(result.recommended).toBe("wait");
      expect(result.actions.some((a) => a.id === "claim")).toBe(false);
      expect(
        result.actions.find((a) => a.id === "refund_unilateral")?.blockedBy
          ?.kind,
      ).toBe("timelock_not_expired");
    });

    it("claim window closed, refund unlocked → refund recommended, never claim", () => {
      const result = deriveSwapActions(
        input({ ...base, serverChainNow: 25_000, clientChainNow: 25_000 }),
      );
      expect(result.recommended).toBe("refund_unilateral");
      expect(result.actions.some((a) => a.id === "claim")).toBe(false);
      expect(result.actions[0]?.blockedBy).toBeUndefined();
    });
  });

  describe("clientredeeming", () => {
    it("→ wait for the claim to confirm", () => {
      expect(deriveSwapActions(input({ status: "clientredeeming" }))).toEqual({
        recommended: "wait",
        actions: [
          {
            id: "wait",
            recommended: true,
            automation: "auto",
            reason: expect.any(String),
          },
        ],
      });
    });
  });

  describe("terminal successes", () => {
    it.each([
      "clientredeemed",
      "serverredeemed",
    ] as const)("%s → terminal none action (funds received, nothing to do)", (status) => {
      expect(deriveSwapActions(input({ status }))).toEqual({
        recommended: "none",
        actions: [
          {
            id: "none",
            recommended: true,
            automation: "auto",
            reason: expect.any(String),
          },
        ],
      });
    });
  });

  describe("expired", () => {
    it("→ terminal none (expired without funding, nothing locked)", () => {
      expect(deriveSwapActions(input({ status: "expired" }))).toEqual({
        recommended: "none",
        actions: [
          {
            id: "none",
            recommended: true,
            automation: "auto",
            reason: expect.any(String),
          },
        ],
      });
    });
  });

  describe("failure → refund", () => {
    const states = [
      "clientfundedserverrefunded",
      "serverwontfund",
      "clientfundedtoolate",
      "clientinvalidfunded",
    ] as const;

    it.each(states)("%s, refund unlocked → refund recommended", (status) => {
      const result = deriveSwapActions(
        input({ status, clientChainNow: 20_000, clientRefundLocktime: 10_000 }),
      );
      expect(result.recommended).toBe("refund_unilateral");
      expect(result.actions).toHaveLength(1);
      expect(result.actions[0]?.blockedBy).toBeUndefined();
    });

    it.each(
      states,
    )("%s, refund still locked → wait + blocked refund (no claim/fund)", (status) => {
      const result = deriveSwapActions(
        input({ status, clientChainNow: 1_000, clientRefundLocktime: 10_000 }),
      );
      expect(result.recommended).toBe("wait");
      expect(result.actions.some((a) => a.id === "claim")).toBe(false);
      expect(
        result.actions.find((a) => a.id === "refund_unilateral")?.blockedBy
          ?.kind,
      ).toBe("timelock_not_expired");
    });
  });

  describe("terminal refunds / errors", () => {
    it.each([
      "clientrefunded",
      "clientrefundedserverrefunded",
      "clientredeemedandclientrefunded",
      "clientrefundedserverfunded",
    ] as const)("%s → terminal none (nothing to do)", (status) => {
      expect(deriveSwapActions(input({ status }))).toEqual({
        recommended: "none",
        actions: [
          {
            id: "none",
            recommended: true,
            automation: "auto",
            reason: expect.any(String),
          },
        ],
      });
    });
  });

  // Guard: funding may only ever be recommended for a not-yet-expired `pending`
  // swap. No other status — and no expired `pending` — may suggest funding, now
  // or as more states get modelled.
  describe("never recommends funding otherwise", () => {
    const nonPending = ALL_STATUSES.filter((s) => s !== "pending");

    it.each(nonPending)("%s → no fund action", (status) => {
      const result = deriveSwapActions(input({ status }));
      expect(result.recommended).not.toBe("fund");
      expect(result.actions.some((a) => a.id === "fund")).toBe(false);
    });

    it("pending past its locktime → no fund action", () => {
      const result = deriveSwapActions(
        input({ status: "pending", clientChainNow: 20_000 }),
      );
      expect(result.recommended).not.toBe("fund");
      expect(result.actions.some((a) => a.id === "fund")).toBe(false);
    });
  });

  // Pay-on-Lightning (clientFunds: false): the client's deposit is an off-chain
  // Lightning payment, so there is no on-chain fund to recommend and nothing to
  // unilaterally refund — the Lightning wallet unwinds a hold invoice itself.
  describe("pay-on-Lightning (clientFunds: false)", () => {
    it("pending → wait (never fund; the invoice is paid off-chain)", () => {
      const result = deriveSwapActions(
        input({ status: "pending", clientFunds: false }),
      );
      expect(result.recommended).toBe("wait");
      expect(result.actions.some((a) => a.id === "fund")).toBe(false);
    });

    it("serverfunded within the claim window → claim (as usual)", () => {
      expect(
        deriveSwapActions(input({ status: "serverfunded", clientFunds: false }))
          .recommended,
      ).toBe("claim");
    });

    it("serverfunded past the claim window → done, not refund (nothing on-chain to reclaim)", () => {
      const result = deriveSwapActions(
        input({
          status: "serverfunded",
          clientFunds: false,
          serverChainNow: 20_000, // claim window closed
        }),
      );
      expect(result.recommended).toBe("none");
      expect(result.actions.some((a) => a.id === "refund_unilateral")).toBe(
        false,
      );
    });
  });
});
