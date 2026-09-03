import type { SwapStatus } from "@lendasat/lendaswap-sdk-pure";
import { describe, expect, it } from "vitest";
import { deriveSwapActions, FUNDING_REAP_GRACE_MS } from "./derive.js";
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

    it("at the client refund locktime → no longer fundable, but still watched", () => {
      const result = deriveSwapActions(
        input({ status: "pending", clientChainNow: 10_000 }),
      );
      // Not yet terminal: `pending` only means the last read saw no deposit — a
      // deadline-straddling funding may still surface and need the refund path,
      // so the swap keeps being watched through the grace window.
      expect(result.actions).toEqual([]);
      expect(result.recommended).toBeUndefined();
    });

    it("past the locktime + grace → dead swap, derives none (reapable)", () => {
      const result = deriveSwapActions(
        input({
          status: "pending",
          clientChainNow: 10_000 + FUNDING_REAP_GRACE_MS,
        }),
      );
      // `none`, not an empty set: nothing was funded in the whole grace window,
      // so the swap is terminal and the tracker must be able to reap it.
      expect(result.recommended).toBe("none");
      expect(result.actions).toEqual([
        {
          id: "none",
          outcome: "expired",
          recommended: true,
          automation: "auto",
          reason: expect.any(String),
        },
      ]);
      expect(result.actions.some((a) => a.id === "fund")).toBe(false);
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
              waitingOn: "client_funding_confirmation",
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
            waitingOn: "server_funding",
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
            waitingOn: "claim_confirmation",
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
            outcome: "completed",
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
            outcome: "expired",
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
      ["clientrefunded", "refunded"],
      ["clientrefundedserverrefunded", "refunded"],
      ["clientrefundedserverfunded", "refunded"],
      // Anomalous both-sides state: the client holds funds — completed from its view.
      ["clientredeemedandclientrefunded", "completed"],
    ] as const)("%s → terminal none (outcome %s)", (status, outcome) => {
      expect(deriveSwapActions(input({ status }))).toEqual({
        recommended: "none",
        actions: [
          {
            id: "none",
            outcome,
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

  // Receive-on-Lightning (serverFunds: false): the server's leg is an off-chain
  // payment to the client's invoice, so "serverfunded" is a payment in flight —
  // nothing to claim, no claim window, and the serverRefundLocktime the tracker
  // reports (0) must not read as "claim window closed".
  describe("receive-on-Lightning (serverFunds: false)", () => {
    it("serverfunded → wait on the payment, refund listed but blocked (never claim, never refund early)", () => {
      const result = deriveSwapActions(
        input({
          status: "serverfunded",
          serverFunds: false,
          serverRefundLocktime: 0,
        }),
      );
      expect(result.recommended).toBe("wait");
      expect(result.actions[0]).toMatchObject({
        id: "wait",
        waitingOn: "server_funding",
      });
      expect(result.actions.some((a) => a.id === "claim")).toBe(false);
      expect(result.actions[1]).toMatchObject({
        id: "refund_unilateral",
        recommended: false,
        blockedBy: { kind: "timelock_not_expired" },
      });
    });

    it("serverfunded past the client timelock → refund", () => {
      const result = deriveSwapActions(
        input({
          status: "serverfunded",
          serverFunds: false,
          serverRefundLocktime: 0,
          clientChainNow: 20_000,
        }),
      );
      expect(result.recommended).toBe("refund_unilateral");
    });

    it("clientfunded is unchanged: wait on the server, refund blocked", () => {
      const result = deriveSwapActions(
        input({
          status: "clientfunded",
          serverFunds: false,
          serverRefundLocktime: 0,
        }),
      );
      expect(result.recommended).toBe("wait");
      expect(result.actions[0]).toMatchObject({
        id: "wait",
        waitingOn: "server_funding",
      });
    });
  });

  // Pay-on-Lightning (clientFunds: false): the client's deposit is an off-chain
  // Lightning payment, so there is no on-chain fund to recommend and nothing to
  // unilaterally refund — the Lightning wallet unwinds a hold invoice itself.
  describe("pay-on-Lightning (clientFunds: false)", () => {
    it("pending → wait on the client's payment (never fund; the invoice is paid off-chain)", () => {
      const result = deriveSwapActions(
        input({ status: "pending", clientFunds: false }),
      );
      expect(result.recommended).toBe("wait");
      // Machine-readable: a consumer routes to its invoice/payment UI on this,
      // instead of inferring "lightning-funded" from status + observations.
      expect(result.actions[0]).toMatchObject({
        id: "wait",
        waitingOn: "client_payment",
      });
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
      expect(result.actions[0]).toMatchObject({
        id: "none",
        outcome: "refunded",
      });
      expect(result.actions.some((a) => a.id === "refund_unilateral")).toBe(
        false,
      );
    });

    it("clientfunded (payment locked in) → wait on server funding, never refund", () => {
      // clientRefundLocktime is 0 for pay-on-Lightning swaps, so without the
      // clientFunds guard the refund would look permanently unlocked.
      const result = deriveSwapActions(
        input({
          status: "clientfunded",
          clientFunds: false,
          clientRefundLocktime: 0,
        }),
      );
      expect(result.recommended).toBe("wait");
      expect(result.actions[0]).toMatchObject({
        id: "wait",
        waitingOn: "server_funding",
      });
      expect(result.actions.some((a) => a.id === "refund_unilateral")).toBe(
        false,
      );
    });

    it.each([
      "clientfundedserverrefunded",
      "serverwontfund",
      "clientfundedtoolate",
      "clientinvalidfunded",
    ] as const)("%s → terminal none (the Lightning payment unwinds on its own)", (status) => {
      const result = deriveSwapActions(
        input({ status, clientFunds: false, clientRefundLocktime: 0 }),
      );
      expect(result.recommended).toBe("none");
      expect(result.actions[0]).toMatchObject({
        id: "none",
        outcome: "refunded",
      });
      expect(result.actions.some((a) => a.id === "refund_unilateral")).toBe(
        false,
      );
    });
  });

  // The refund-blocked waits carry `refund_timelock` — a consumer shows the
  // countdown/refund surface, not a generic processing spinner.
  describe("waitingOn: refund_timelock", () => {
    it("serverfunded with the claim window closed", () => {
      const result = deriveSwapActions(
        input({
          status: "serverfunded",
          clientRefundLocktime: 20_000,
          serverChainNow: 15_000,
        }),
      );
      expect(result.actions[0]).toMatchObject({
        id: "wait",
        waitingOn: "refund_timelock",
      });
    });

    it("failed statuses while the refund is still locked", () => {
      const result = deriveSwapActions(
        input({ status: "clientinvalidfunded", clientChainNow: 1_000 }),
      );
      expect(result.actions[0]).toMatchObject({
        id: "wait",
        waitingOn: "refund_timelock",
      });
    });
  });
});
