import { describe, expect, it } from "vitest";
import { assertPaymentClaimWindow } from "../src/create/payment-safety.js";

describe("payment preimage disclosure", () => {
  it("allows an authorized replacement near expiry after the first broadcast attempt", () => {
    const now = 1800000000;
    assertPaymentClaimWindow("mainnet", now + 900, false, now);
    expect(() =>
      assertPaymentClaimWindow("mainnet", now + 900, false, now + 400),
    ).toThrow("preimage");
    expect(() =>
      assertPaymentClaimWindow("mainnet", now + 900, true, now + 400),
    ).not.toThrow();
  });
  it.each([
    "signet",
    "testnet",
    "regtest",
  ])("supports the two-minute %s timeout but refuses a first reveal at expiry", (network) => {
    expect(() =>
      assertPaymentClaimWindow(network, 120, false, 0),
    ).not.toThrow();
    expect(() => assertPaymentClaimWindow(network, 120, false, 100)).toThrow(
      "preimage",
    );
  });
});
