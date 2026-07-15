import type { SwapStatus } from "@lendasat/lendaswap-sdk-pure";
import { describe, expect, it } from "vitest";
import { deriveSwapStatus } from "./status.js";
import type { HtlcObservation } from "./types.js";

describe("deriveSwapStatus", () => {
  // [clientHtlc, serverHtlc, expected]
  const cases: Array<
    [HtlcObservation, HtlcObservation, SwapStatus | undefined]
  > = [
    // Funding progression.
    ["absent", "absent", "pending"],
    ["mempool", "absent", "clientfundingseen"],
    ["confirmed", "absent", "clientfunded"],
    ["confirmed", "mempool", "clientfunded"], // server funding unconfirmed → still wait
    ["confirmed", "confirmed", "serverfunded"],
    // Amount/terms mismatch: the client's own leg funded invalidly → reclaim it.
    ["invalid", "absent", "clientinvalidfunded"],
    ["invalid", "confirmed", "clientinvalidfunded"],
    // A server leg funded on the wrong terms is NOT serverfunded — so the client
    // never claims it; it stays clientfunded and refunds after its timelock.
    ["confirmed", "invalid", "clientfunded"],
    // Success.
    ["confirmed", "spent_claim", "clientredeemed"],
    ["spent_claim", "spent_claim", "serverredeemed"],
    // Refund / failure.
    ["spent_refund", "absent", "clientrefunded"],
    ["spent_refund", "mempool", "clientrefunded"],
    ["confirmed", "spent_refund", "clientfundedserverrefunded"],
    ["spent_refund", "confirmed", "clientrefundedserverfunded"], // error state
    ["spent_refund", "spent_refund", "clientrefundedserverrefunded"],
    ["spent_refund", "spent_claim", "clientredeemedandclientrefunded"], // anomaly
    // Contradictory — server can't hold the preimage before the client reveals it.
    ["spent_claim", "absent", undefined],
    ["spent_claim", "confirmed", undefined],
    ["spent_claim", "spent_refund", undefined],
  ];

  it.each(
    cases,
  )("client=%s, server=%s → %s", (clientHtlc, serverHtlc, expected) => {
    expect(deriveSwapStatus({ clientHtlc, serverHtlc })).toBe(expected);
  });

  // Receive-on-Lightning (*_to_lightning): only the client's on-chain leg exists;
  // the server sweeping it (spent_claim) is the success signal.
  describe("receive-on-Lightning (serverHtlc absent)", () => {
    const cases: Array<[HtlcObservation, SwapStatus]> = [
      ["absent", "pending"],
      ["mempool", "clientfundingseen"],
      ["confirmed", "clientfunded"], // waiting for the server to pay + sweep
      ["invalid", "clientinvalidfunded"],
      ["spent_claim", "serverredeemed"], // server swept ⟹ it paid the invoice
      ["spent_refund", "clientrefunded"],
    ];
    it.each(cases)("client=%s → %s", (clientHtlc, expected) => {
      expect(deriveSwapStatus({ clientHtlc })).toBe(expected);
    });
  });

  // Pay-on-Lightning (lightning_to_*): only the on-chain leg the client claims
  // exists; the client's claim (spent_claim) completes the swap.
  describe("pay-on-Lightning (clientHtlc absent)", () => {
    const cases: Array<[HtlcObservation, SwapStatus]> = [
      ["absent", "pending"], // invoice not yet landed as a funded HTLC
      ["mempool", "pending"],
      ["invalid", "pending"], // wrong terms — must not claim; LN unwinds
      ["confirmed", "serverfunded"], // ready to claim
      ["spent_claim", "clientredeemed"], // client claimed ⟹ complete
      ["spent_refund", "clientrefunded"], // server reclaimed; LN payment unwinds
    ];
    it.each(cases)("server=%s → %s", (serverHtlc, expected) => {
      expect(deriveSwapStatus({ serverHtlc })).toBe(expected);
    });
  });

  it("returns undefined when neither leg is present", () => {
    expect(deriveSwapStatus({})).toBeUndefined();
  });
});
