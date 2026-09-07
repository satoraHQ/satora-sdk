import { afterEach, describe, expect, it, vi } from "vitest";
import {
  Client,
  type GetSwapResponse,
  InMemorySwapStorage,
  SWAP_STORAGE_VERSION,
} from "../src/index.js";

const COORDINATOR = "0x4444444444444444444444444444444444444444";
const HTLC = "0x5555555555555555555555555555555555555555";
const SERVER = "0x3333333333333333333333333333333333333333";
const WBTC_POLYGON = "0x1bfd67037b42cf73acf2047067bd4f2c47d9bfd6";
const CALLDATA = "0xdeadbeef";

/**
 * A stored EVM-sourced swap whose source token is the BTC-pegged lock token
 * itself (no DEX leg). Only the fields `refundSwap` reads are populated.
 */
function storedSwap(
  id: string,
  direction: "evm_to_arkade" | "evm_to_bitcoin" | "evm_to_lightning",
) {
  const response = {
    id,
    direction,
    status: "clientinvalidfunded",
    evm_htlc_address: HTLC,
    evm_coordinator_address: COORDINATOR,
    server_evm_address: SERVER,
    evm_refund_locktime: 1_800_000_000,
    source_amount: "100250",
    source_token: {
      token_id: WBTC_POLYGON,
      symbol: "WBTC",
      chain: "137",
      decimals: 8,
    },
  } as unknown as GetSwapResponse;
  return {
    version: SWAP_STORAGE_VERSION,
    swapId: id,
    keyIndex: 0,
    response,
    publicKey: `02${"11".repeat(32)}`,
    preimage: "22".repeat(32),
    preimageHash: "33".repeat(32),
    secretKey: "44".repeat(32),
    storedAt: 0,
    updatedAt: 0,
  };
}

async function refundFor(
  direction: "evm_to_arkade" | "evm_to_bitcoin" | "evm_to_lightning",
) {
  const id = `swap-${direction}`;
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const url = new URL((input as Request).url);
    if (url.pathname === `/swap/${id}/refund-and-swap-calldata`) {
      return Response.json(
        { coordinator_address: COORDINATOR, calldata: CALLDATA },
        { status: 200 },
      );
    }
    return Response.json({ error: "unexpected request" }, { status: 404 });
  });
  vi.stubGlobal("fetch", fetchMock);

  const storage = new InMemorySwapStorage();
  await storage.store(storedSwap(id, direction));
  const client = await Client.builder()
    .withBaseUrl("https://example.test")
    .withSwapStorage(storage)
    .build();

  const result = await client.refundSwap(id);
  const paths = fetchMock.mock.calls.map(
    ([input]) => new URL((input as Request).url).pathname,
  );
  return { result, paths };
}

/// Every EVM-source funding is created by the coordinator, which is the
/// HTLC sender; a direct `HTLCErc20.refund` from the wallet hashes to a
/// different key and reverts. So even a BTC-pegged source must refund via
/// the coordinator's `refundTo`.
describe("refundSwap for a BTC-pegged EVM source", () => {
  afterEach(() => vi.unstubAllGlobals());

  for (const direction of [
    "evm_to_arkade",
    "evm_to_bitcoin",
    "evm_to_lightning",
  ] as const) {
    it(`targets the coordinator for ${direction}`, async () => {
      const { result, paths } = await refundFor(direction);
      expect(result.success).toBe(true);
      expect(result.evmRefundData?.to).toBe(COORDINATOR);
      expect(result.evmRefundData?.data).toBe(CALLDATA);
      expect(paths).toEqual([
        `/swap/swap-${direction}/refund-and-swap-calldata`,
      ]);
    });
  }
});
