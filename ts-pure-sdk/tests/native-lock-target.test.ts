import { describe, expect, it } from "vitest";
import {
  getBridgeTargetChain,
  isBridgeOnlyChain,
  isNativeLockTarget,
} from "../src/tokens.js";
import { USDT0_ADDRESSES } from "../src/usdt0-bridge/index.js";

const ZERO = "0x0000000000000000000000000000000000000000";

/**
 * Rootstock is both a USDT0 bridge destination (via the Arbitrum hub) and the
 * chain that locks its own coin in HTLCNative. The token decides which.
 */
describe("isNativeLockTarget", () => {
  it("is RBTC on chain 30 only", () => {
    expect(isNativeLockTarget("30", ZERO)).toBe(true);
    expect(isNativeLockTarget("rootstock", ZERO)).toBe(true);
    expect(isNativeLockTarget("30", USDT0_ADDRESSES.Rootstock)).toBe(false);
    expect(isNativeLockTarget("42161", ZERO)).toBe(false);
  });

  it("keeps Rootstock a bridge-only chain for its other tokens", () => {
    expect(isBridgeOnlyChain("30")).toBe(true);
    expect(
      getBridgeTargetChain({
        chain: "30",
        token_id: USDT0_ADDRESSES.Rootstock,
      } as Parameters<typeof getBridgeTargetChain>[0]),
    ).toBe("Rootstock");
    expect(
      getBridgeTargetChain({
        chain: "30",
        token_id: ZERO,
      } as Parameters<typeof getBridgeTargetChain>[0]),
    ).toBeUndefined();
  });
});
