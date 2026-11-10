import { describe, expect, it } from "vitest";
import { fundingTermsOf } from "../src/create/funding-terms.js";

const response = {
  evm_chain_id: 137,
  evm_coordinator_address: "0xCoordinator",
  evm_htlc_address: "0xHtlc",
  server_evm_address: "0xServer",
  evm_refund_locktime: 2_000_000_000,
  source_amount: "100000",
  source_token: { token_id: "0xSource" },
};

describe("fundingTermsOf lock token", () => {
  it("reads evm_htlc_token over the deprecated wbtc_address", () => {
    const terms = fundingTermsOf({
      ...response,
      evm_htlc_token: "0xLockToken",
      wbtc_address: "0xStale",
    });
    expect(terms?.lockToken).toBe("0xlocktoken");
  });

  it("falls back to wbtc_address from servers before evm_htlc_token", () => {
    expect(
      fundingTermsOf({ ...response, wbtc_address: "0xWbtc" })?.lockToken,
    ).toBe("0xwbtc");
  });

  it("names no lock token when the response has neither", () => {
    expect(fundingTermsOf(response)?.lockToken).toBeUndefined();
  });
});
