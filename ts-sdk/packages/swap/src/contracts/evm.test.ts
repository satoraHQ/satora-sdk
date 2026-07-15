import { describe, expect, it } from "vitest";
import { type EvmHtlcEvent, evmObservation } from "./evm.js";

const TOKEN = "0xWBTC" as const;
const created: EvmHtlcEvent = { kind: "created", amount: 1000n, token: TOKEN };
const redeemed: EvmHtlcEvent = { kind: "redeemed", preimage: "0xpre" };
const refunded: EvmHtlcEvent = { kind: "refunded" };

const expect1000 = { amount: 1000n, token: TOKEN };

describe("evmObservation", () => {
  it("is absent with no events, mempool while a funding tx is pending", () => {
    expect(evmObservation([], expect1000)).toEqual({ observation: "absent" });
    expect(evmObservation([], expect1000, true)).toEqual({
      observation: "mempool",
    });
  });

  it("is confirmed once funded with the expected amount + token", () => {
    expect(evmObservation([created], expect1000)).toEqual({
      observation: "confirmed",
    });
    // over-funded is still safe to claim
    expect(
      evmObservation(
        [{ kind: "created", amount: 2000n, token: TOKEN }],
        expect1000,
      ),
    ).toEqual({ observation: "confirmed" });
  });

  it("is invalid when funded below the expected amount", () => {
    expect(
      evmObservation(
        [{ kind: "created", amount: 999n, token: TOKEN }],
        expect1000,
      ),
    ).toEqual({ observation: "invalid" });
  });

  it("is invalid when the locked token is not the expected one", () => {
    expect(
      evmObservation(
        [{ kind: "created", amount: 1000n, token: "0xEVIL" }],
        expect1000,
      ),
    ).toEqual({ observation: "invalid" });
  });

  it("skips the token check when no expected token is given", () => {
    expect(
      evmObservation(
        [{ kind: "created", amount: 1000n, token: "0xanything" }],
        {
          amount: 1000n,
        },
      ),
    ).toEqual({ observation: "confirmed" });
  });

  it("reports a claim with the revealed preimage", () => {
    expect(evmObservation([created, redeemed], expect1000)).toEqual({
      observation: "spent_claim",
      preimage: "0xpre",
    });
  });

  it("reports a timelock refund", () => {
    expect(evmObservation([created, refunded], expect1000)).toEqual({
      observation: "spent_refund",
    });
  });

  it("lets a claim win over a stray refund event, order-independently", () => {
    expect(evmObservation([refunded, redeemed, created], expect1000)).toEqual({
      observation: "spent_claim",
      preimage: "0xpre",
    });
  });
});
