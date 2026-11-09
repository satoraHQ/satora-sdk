import { describe, expect, it } from "vitest";
import type { TokenInfo } from "../src/api/client.js";
import { getBridgeTargetChain, isBridgedTarget } from "../src/tokens.js";

const USDC_POLYGON = "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359";
const USDC_ETHEREUM = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
const USDC_ARBITRUM = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831";
const USDT_POLYGON = "0xc2132D05D31c914a87C6611C10748AEb04B58e8F";
const WBTC_POLYGON = "0x1bfd67037b42cf73acf2047067bd4f2c47d9bfd6";

describe("USDC payouts on Ethereum and Polygon bridge over CCTP", () => {
  it("bridges USDC on Polygon and Ethereum, by id or name, any case", () => {
    expect(isBridgedTarget("137", USDC_POLYGON)).toBe(true);
    expect(isBridgedTarget("1", USDC_ETHEREUM.toLowerCase())).toBe(true);
    expect(isBridgedTarget("Polygon", USDC_POLYGON)).toBe(true);
  });

  it("keeps other hub tokens and Arbitrum USDC direct", () => {
    expect(isBridgedTarget("137", USDT_POLYGON)).toBe(false);
    expect(isBridgedTarget("137", WBTC_POLYGON)).toBe(false);
    expect(isBridgedTarget("42161", USDC_ARBITRUM)).toBe(false);
    // Polygon's USDC address on Ethereum is not Ethereum's USDC.
    expect(isBridgedTarget("1", USDC_POLYGON)).toBe(false);
  });

  it("still bridges bridge-only chains but not RBTC", () => {
    expect(
      isBridgedTarget("8453", "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
    ).toBe(true);
    expect(
      isBridgedTarget("30", "0x0000000000000000000000000000000000000000"),
    ).toBe(false);
  });

  it("names the CCTP destination for a USDC token", () => {
    const token = {
      token_id: USDC_POLYGON,
      symbol: "USDC",
      name: "USD Coin",
      decimals: 6,
      chain: "137",
    } as unknown as TokenInfo;
    expect(getBridgeTargetChain(token)).toBe("Polygon");
  });
});
