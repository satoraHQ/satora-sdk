import { describe, expect, it } from "vitest";
import {
  fromWireDexQuoteResponse,
  type WireDexQuoteResponse,
} from "../src/types/dex-quote.js";

function quote(self_relayed?: boolean | null): WireDexQuoteResponse {
  return {
    expected_amount_in: { raw: "100000", decimals: 8 },
    estimated_amount_out: { raw: "42000000", decimals: 6 },
    estimated_settlement_seconds: 30,
    router: "1inch",
    cache_ttl_seconds: 10,
    bridge_rate: {
      router: "cctp",
      fee_token: {
        kind: "evm",
        chain_id: 137,
        address: "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359",
      },
      proportional_fee_millionths: "130",
      flat: self_relayed ? "0" : "200000",
      flat_with_setup: self_relayed ? "0" : "200000",
      ...(self_relayed === undefined ? {} : { self_relayed }),
    },
  };
}

function selfRelayed(wire: WireDexQuoteResponse): boolean | undefined {
  const rate = fromWireDexQuoteResponse(wire).bridge_rate;
  if (rate?.router !== "cctp") throw new Error("expected a CCTP bridge rate");
  return rate.self_relayed;
}

describe("fromWireDexQuoteResponse", () => {
  it("keeps who delivers a CCTP payout", () => {
    expect(selfRelayed(quote(true))).toBe(true);
    expect(selfRelayed(quote(false))).toBe(false);
  });

  it("leaves it undefined for a server that does not say", () => {
    expect(selfRelayed(quote())).toBeUndefined();
    expect(selfRelayed(quote(null))).toBeUndefined();
  });
});
