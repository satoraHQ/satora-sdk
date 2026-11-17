import { describe, expect, it } from "vitest";
import {
  type ComposeQuoteDeps,
  type ComposeQuoteParams,
  composeQuote,
} from "../src/compose-quote.js";
import type { Chain } from "../src/types/index.js";

const ARB_TBTC = "0x6c84a8f1c29108f47a79964b5fe888d4f4d0de40";
const POLYGON_USDC = "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359";
const NETWORK_FEE_SATS = 500;
const FORWARDING_FEE = 200_000n;

// 100k sats of tBTC (18 dec) buys 100 USDC (6 dec) on the DEX leg.
const DEX_OUT = 100_000_000n;
const SAT_IN_TBTC_BASE = 10_000_000_000n;
const FULL_IN = 100_000n * SAT_IN_TBTC_BASE;

/**
 * A server that delivers a payout itself only at the full amount: with the
 * BTC-side fees off it falls under its minimum and Circle forwards it.
 */
function deps(selfRelayedNet: boolean): ComposeQuoteDeps {
  const pairs = [{ source: "Bitcoin", target: "42161" }] as const;
  return {
    swapPairs: {
      pairs: pairs.map((p) => ({
        ...p,
        min_sats: 1000,
        max_sats: 100_000_000,
        fee_percentage: 0,
        network_fee_sats: 0,
      })),
    },
    networkFees: {
      pairs: pairs.map((p) => ({
        ...p,
        fees: { source_sats: NETWORK_FEE_SATS, target_sats: 0 },
      })),
    },
    chainConfig: {
      chains: [
        {
          chain: "42161",
          btc_pegged_token: { address: ARB_TBTC, decimals: 18, symbol: "tBTC" },
          btc_peg_rate: "1",
        },
      ],
    },
    getDexQuote: async ({ amount }) => {
      const selfRelayed = BigInt(amount.value) >= FULL_IN || selfRelayedNet;
      const fee = selfRelayed ? 0n : FORWARDING_FEE;
      const gross = (BigInt(amount.value) * DEX_OUT) / FULL_IN;
      return {
        expected_amount_in: { raw: amount.value, decimals: 18 },
        estimated_amount_out: { raw: (gross - fee).toString(), decimals: 6 },
        bridge_fee: fee.toString(),
        bridge_rate: {
          router: "cctp",
          self_relayed: selfRelayed,
        } as never,
      };
    },
  };
}

const sourcePinned: ComposeQuoteParams = {
  sourceChain: "Bitcoin",
  sourceToken: "btc",
  targetChain: "137" as Chain,
  targetToken: POLYGON_USDC,
  sourceAmount: 100_000,
};

const effectiveSats = 100_000n - BigInt(NETWORK_FEE_SATS);
const netGross = (effectiveSats * DEX_OUT) / 100_000n;

describe("composeQuote — CCTP payout the server delivers", () => {
  it("charges the forwarding fee when the payout after BTC-side fees is not delivered by the server", async () => {
    const q = await composeQuote(sourcePinned, deps(false));

    expect(q.bridge_fee).toBe(Number(FORWARDING_FEE));
    expect(q.target_amount).toBe(DEX_OUT.toString());
    expect(q.net_target_amount).toBe((netGross - FORWARDING_FEE).toString());
  });

  it("keeps it free when the server still delivers after the fees", async () => {
    const q = await composeQuote(sourcePinned, deps(true));

    expect(q.bridge_fee).toBeUndefined();
    expect(q.net_target_amount).toBe(netGross.toString());
  });
});
