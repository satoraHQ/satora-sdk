import { afterEach, describe, expect, it, vi } from "vitest";
import { computeCoordinatorCallsHash } from "../src/evm/coordinator.js";
import {
  claimViaGasless,
  type GaslessSwapResponse,
} from "../src/redeem/gasless.js";

const WBTC = "0x1111111111111111111111111111111111111111";
const USDC = "0x2222222222222222222222222222222222222222";
const SERVER = "0x3333333333333333333333333333333333333333";

const swap = (targetToken: string) =>
  ({
    id: "8d40ac5a-3b2a-4c1a-ba2e-5448ec6dcd34",
    evm_htlc_kind: "erc20",
    evm_htlc_address: "0x18123a31261a76c74c82ed2f8346002caa78ab39",
    evm_coordinator_address: "0x447a4cd711ce66d25c7008d26d5f09ca7d621f48",
    evm_chain_id: 137,
    evm_expected_sats: "100000",
    server_evm_address: SERVER,
    evm_refund_locktime: 1_800_000_000,
    evm_htlc_version: 4,
    wbtc_address: WBTC,
    target_token: { token_id: targetToken },
  }) as unknown as GaslessSwapResponse;

const drain = computeCoordinatorCallsHash([
  {
    target: WBTC,
    value: 0n,
    data: `0xa9059cbb${SERVER.slice(2).padStart(64, "0")}${100000n.toString(16).padStart(64, "0")}`,
  },
]);

function claim(targetToken: string, callsHash: string) {
  return claimViaGasless({
    baseUrl: "https://example.test",
    preimage: `0x${"11".repeat(32)}`,
    secretKey: new Uint8Array(32).fill(0x44),
    swap: swap(targetToken),
    destination: "0x7564105E977516C53bE337314c7E53838967bDaC",
    minAmountOut: 0n,
    callsHash,
  });
}

function stubRelay() {
  const fetchMock = vi.fn(async () =>
    Response.json({
      id: "8d40ac5a-3b2a-4c1a-ba2e-5448ec6dcd34",
      status: "client_redeeming",
      tx_hash: "0xrelayed",
      message: "ok",
    }),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

afterEach(() => vi.unstubAllGlobals());

describe("direct claim calls", () => {
  it("refuses server calls on a direct WBTC claim before anything is sent", async () => {
    const fetchMock = stubRelay();
    await expect(claim(WBTC, drain)).rejects.toThrow(
      /server calls on a direct claim/,
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("signs a direct WBTC claim with no calls", async () => {
    const fetchMock = stubRelay();
    await expect(
      claim(WBTC, computeCoordinatorCallsHash([])),
    ).resolves.toMatchObject({ txHash: "0xrelayed" });
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("leaves a swap claim's calls to the server", async () => {
    const fetchMock = stubRelay();
    await expect(claim(USDC, drain)).resolves.toMatchObject({
      txHash: "0xrelayed",
    });
    expect(fetchMock).toHaveBeenCalledOnce();
  });
});
