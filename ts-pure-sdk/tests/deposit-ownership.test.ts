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
      "Cannot export a deposit key",
    );
  });

  it.each([
    "shared",
    "per-swap",
    "external",
  ])("exports the compatible key for non-gasless %s records", async (account) => {
    const { client, storage, record } = await setup();
    Object.assign(record.response, {
      gasless: false,
      client_evm_address:
        account === "shared"
          ? client.getEvmAddress()
          : account === "per-swap"
            ? deriveEvmAddress(record.secretKey)
            : "0x1111111111111111111111111111111111111111",
    });
    await storage.store(record);
    const exported = await client.getSwapDepositorKey(record.swapId);
    expect(exported.privateKey).toBe(
      account === "per-swap"
        ? `0x${record.secretKey}`
        : client.getEvmDepositorKey().privateKey,
    );
  });

  it("preserves the shared-key fallback for gasless records with no address", async () => {
    const { client, storage, record } = await setup();
    Object.assign(record.response, { client_evm_address: undefined });
    await storage.store(record);
    expect(await client.getSwapDepositorKey(record.swapId)).toEqual(
      client.getEvmDepositorKey(),
    );
  });

  const canonicalId = "3f2504e0-4f89-11d3-9a0c-0305e82c3301";
  it.each([
    canonicalId.toUpperCase(),
    canonicalId.replace(/-/g, ""),
    `{${canonicalId}}`,
    `urn:uuid:${canonicalId}`,
  ])("accepts equivalent UUID %s", async (id) => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => Response.json({ id: canonicalId })),
    );
    const { client } = await setup();
    expect((await client.getSwap(id)).id).toBe(canonicalId);
  });

  it("rejects genuinely different UUIDs", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => Response.json({ id: canonicalId })),
    );
    const { client } = await setup();
    await expect(
      client.getSwap("4f2504e0-4f89-11d3-9a0c-0305e82c3301"),
    ).rejects.toThrow("Requested swap");
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
