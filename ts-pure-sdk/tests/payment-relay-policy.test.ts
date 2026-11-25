import { afterEach, describe, expect, it, vi } from "vitest";
import { fetchMinimumRelayFeeRate } from "../src/esplora.js";

afterEach(() => vi.unstubAllGlobals());
describe("payment relay policy", () => {
  it("accepts fractional policy and does not substitute a confirmation estimate", async () => {
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValue(
          new Response(JSON.stringify({ minimumFee: 0.1, fastestFee: 100 })),
        ),
    );
    expect(await fetchMinimumRelayFeeRate("https://explorer.test/api")).toBe(
      0.1,
    );
  });
  it("returns unknown for an explorer without policy support", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response("", { status: 404 })),
    );
    expect(
      await fetchMinimumRelayFeeRate("https://explorer.test/api"),
    ).toBeUndefined();
  });
});
