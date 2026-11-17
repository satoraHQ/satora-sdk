import { afterEach, describe, expect, it, vi } from "vitest";
import { Client } from "../src/client.js";
import { deriveEvmAddress } from "../src/evm/signing.js";
import { bytesToHex } from "../src/signer/index.js";
import { InMemorySwapStorage } from "../src/storage/index.js";
import type { StoredSwap } from "../src/storage/types.js";

const mnemonic =
  "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

afterEach(() => vi.unstubAllGlobals());

async function setup() {
  const storage = new InMemorySwapStorage();
  const client = await Client.builder()
    .withBaseUrl("https://swap.test")
    .withMnemonic(mnemonic)
    .withSwapStorage(storage)
    .build();
  const params = client.deriveSwapParamsAtIndex(7);
  const record = {
    version: 2,
    swapId: "swap-a",
    keyIndex: 7,
    secretKey: bytesToHex(params.secretKey),
    publicKey: bytesToHex(params.publicKey),
    preimage: bytesToHex(params.preimage),
    preimageHash: bytesToHex(params.preimageHash),
    storedAt: 0,
    updatedAt: 0,
    response: {
      id: "swap-a",
      direction: "evm_to_bitcoin",
      gasless: true,
      client_evm_address: deriveEvmAddress(params.secretKey),
    },
  } as StoredSwap;
  return { client, storage, record };
}

describe("deposit ownership compatibility", () => {
  it("exports the original per-swap depositor rather than the shared key", async () => {
    const { client, storage, record } = await setup();
    await storage.store(record);
    const key = await client.getSwapDepositorKey(record.swapId);
    expect(key.privateKey).toBe(`0x${record.secretKey}`);
    expect(key.address).not.toBe(client.getEvmAddress());
  });

  it("preserves shared-address deposit recovery", async () => {
    const { client, storage, record } = await setup();
    Object.assign(record.response, {
      client_evm_address: client.getEvmAddress(),
    });
    await storage.store(record);
    expect(await client.getSwapDepositorKey(record.swapId)).toEqual(
      client.getEvmDepositorKey(),
    );
  });

  it("does not export an unrelated key for an unknown gasless depositor", async () => {
    const { client, storage, record } = await setup();
    Object.assign(record.response, {
      client_evm_address: "0x1111111111111111111111111111111111111111",
    });
    await storage.store(record);
    await expect(client.getSwapDepositorKey(record.swapId)).rejects.toThrow(
      "Cannot sign for this swap",
    );
  });

  it.each([
    false,
    true,
  ])("rejects a different response ID (updateStorage=%s)", async (updateStorage) => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => Response.json({ id: "swap-b" })),
    );
    const { client, storage, record } = await setup();
    await storage.store(record);
    await expect(client.getSwap("swap-a", { updateStorage })).rejects.toThrow(
      "Requested swap swap-a, but server returned swap-b",
    );
    expect(await storage.get("swap-a")).toEqual(record);
    expect(await storage.get("swap-b")).toBeNull();
  });
});
