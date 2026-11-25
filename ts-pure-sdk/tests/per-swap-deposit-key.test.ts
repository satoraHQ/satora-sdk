import { afterEach, describe, expect, it, vi } from "vitest";
import { Client } from "../src/client.js";
import { createEvmToBitcoinSwap } from "../src/create/evm-to-bitcoin.js";
import type { CreateSwapContext } from "../src/create/types.js";
import { deriveEvmAddress } from "../src/evm/signing.js";
import { bytesToHex } from "../src/signer/index.js";
import { InMemorySwapStorage } from "../src/storage/index.js";
import { storedSwap, swapResponse } from "./evm-funding-fixture.js";

const mnemonic =
  "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
afterEach(() => vi.restoreAllMocks());

describe("per-payment gasless deposits", () => {
  it.each([
    1, 42161,
  ])("isolates simultaneous payments on chain %s", async (chain) => {
    const client = await Client.builder().withMnemonic(mnemonic).build();
    const post = vi.fn(async (_path, { body }) => ({
      data: {
        ...swapResponse(),
        id: body.hash_lock,
        client_evm_address: body.user_address,
        target_btc_address: body.target_address,
      },
    }));
    const context = {
      apiClient: { POST: post },
      evmAddress: client.getEvmAddress(),
      deriveSwapParams: () => client.deriveSwapParams(),
      storeSwap: vi.fn(),
    } as unknown as CreateSwapContext;
    const options = {
      gasless: true,
      evmChainId: chain,
      tokenAddress: "0x0000000000000000000000000000000000000001",
      userAddress: "",
      targetAddress: "recovery",
      sourceAmount: 100n,
    };
    const [a, b] = await Promise.all([
      createEvmToBitcoinSwap(options, context),
      createEvmToBitcoinSwap(options, context),
    ]);
    expect(a.response.client_evm_address).not.toBe(
      b.response.client_evm_address,
    );
    expect(a.response.client_evm_address).toBe(
      deriveEvmAddress(a.swapParams.secretKey),
    );
    expect(b.response.client_evm_address).toBe(
      deriveEvmAddress(b.swapParams.secretKey),
    );
    expect(a.response.client_evm_address).not.toBe(client.getEvmAddress());
  });

  it("exports the right recovery key for new and legacy deposits", async () => {
    const storage = new InMemorySwapStorage();
    const client = await Client.builder()
      .withMnemonic(mnemonic)
      .withSwapStorage(storage)
      .build();
    const params = client.deriveSwapParamsAtIndex(7);
    for (const address of [
      deriveEvmAddress(params.secretKey),
      client.getEvmAddress(),
    ]) {
      const record = storedSwap({
        response: swapResponse({ gasless: true, clientEvmAddress: address }),
      });
      record.secretKey = bytesToHex(params.secretKey);
      await storage.store(record);
      const recovered = await client.getSwapDepositorKey(record.swapId);
      expect(recovered.address).toBe(address);
      expect(deriveEvmAddress(recovered.privateKey)).toBe(address);
    }
  });
});

it("never selects another payment's key from a changed server depositor", async () => {
  const storage = new InMemorySwapStorage();
  const client = await Client.builder()
    .withMnemonic(mnemonic)
    .withSwapStorage(storage)
    .build();
  const a = client.deriveSwapParamsAtIndex(1);
  const b = client.deriveSwapParamsAtIndex(2);
  const otherAddress = deriveEvmAddress(b.secretKey);
  const record = storedSwap({
    response: swapResponse({ gasless: true, clientEvmAddress: otherAddress }),
  });
  record.secretKey = bytesToHex(a.secretKey);
  await storage.store({
    ...record,
    swapId: "other",
    secretKey: bytesToHex(b.secretKey),
  });
  await storage.store(record);
  await expect(client.getSwapDepositorKey(record.swapId)).rejects.toThrow(
    "no SDK key for its recorded depositor",
  );
  await storage.store({
    ...record,
    evmFundingTerms: {
      ...record.evmFundingTerms!,
      depositor: deriveEvmAddress(a.secretKey),
    },
  });
  expect((await client.getSwapDepositorKey(record.swapId)).address).toBe(
    deriveEvmAddress(a.secretKey),
  );
});
