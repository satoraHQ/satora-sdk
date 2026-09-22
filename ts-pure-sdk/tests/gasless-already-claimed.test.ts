import { afterEach, describe, expect, it, vi } from "vitest";
import {
  alreadyClaimedStatus,
  claimViaGasless,
  type GaslessSwapResponse,
} from "../src/redeem/gasless.js";

/** A native-lock swap: the claim shape with the fewest fields. */
const swap = {
  id: "8d40ac5a-3b2a-4c1a-ba2e-5448ec6dcd34",
  evm_htlc_kind: "native",
  evm_htlc_address: "0x18123a31261a76c74c82ed2f8346002caa78ab39",
  evm_coordinator_address: "0x447a4cd711ce66d25c7008d26d5f09ca7d621f48",
  evm_chain_id: 30,
  evm_expected_sats: "1000000000000",
  server_evm_address: "0x3333333333333333333333333333333333333333",
  evm_refund_locktime: 1_800_000_000,
  evm_htlc_version: 1,
  wbtc_address: "0x0000000000000000000000000000000000000000",
  target_token: {
    token_id: "0x0000000000000000000000000000000000000000",
  },
} as unknown as GaslessSwapResponse;

function claim() {
  return claimViaGasless({
    baseUrl: "https://example.test",
    preimage: `0x${"11".repeat(32)}`,
    secretKey: new Uint8Array(32).fill(0x44),
    swap,
    destination: "0x7564105E977516C53bE337314c7E53838967bDaC",
    minAmountOut: 0n,
    callsHash: `0x${"00".repeat(32)}`,
  });
}

afterEach(() => vi.unstubAllGlobals());

/**
 * The server relays the claim once and then rejects every further attempt
 * with "wrong state"; a client whose chain view could not be refreshed keeps
 * asking. That rejection is the claim's success, not a failure to retry.
 */
describe("claimViaGasless after the server already relayed the claim", () => {
  it("returns the post-claim state instead of throwing", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        Response.json(
          {
            error:
              "Swap in wrong state, cannot claim a swap in client_redeeming",
          },
          { status: 400 },
        ),
      ),
    );
    await expect(claim()).resolves.toMatchObject({
      id: swap.id,
      status: "client_redeeming",
    });
  });

  it("still throws on any other rejection", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        Response.json(
          { error: "Swap in wrong state, cannot claim a swap in pending" },
          { status: 400 },
        ),
      ),
    );
    await expect(claim()).rejects.toThrow("Gasless claim failed (400)");
  });

  it("names only the states after a claim", () => {
    expect(
      alreadyClaimedStatus(
        '{"error":"Swap in wrong state, cannot claim a swap in server_redeemed"}',
      ),
    ).toBe("server_redeemed");
    expect(alreadyClaimedStatus("cannot claim a swap in expired")).toBe(
      undefined,
    );
  });
});
