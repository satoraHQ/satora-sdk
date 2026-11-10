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
  it("reads the lock token from evm_htlc_token", () => {
    const terms = fundingTermsOf({
      ...response,
      evm_htlc_token: "0xLockToken",
    });
    expect(terms?.lockToken).toBe("0xlocktoken");
  });

  it("names no lock token when the response has none", () => {
    expect(fundingTermsOf(response)?.lockToken).toBeUndefined();
  });
});
