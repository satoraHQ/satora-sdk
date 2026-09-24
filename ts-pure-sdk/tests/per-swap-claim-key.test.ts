import { describe, expect, it, vi } from "vitest";
import { Client } from "../src/client.js";
import { createLightningToEvmSwap } from "../src/create/lightning-to-evm.js";
import type { CreateSwapContext } from "../src/create/types.js";
import { deriveEvmAddress } from "../src/evm/signing.js";

const MNEMONIC =
  "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

describe("inbound swaps claim with a per-swap key", () => {
  it("sends the swap key's address as claiming_address, not the wallet-level one", async () => {
    const client = await Client.builder().withMnemonic(MNEMONIC).build();
    const swapParams = client.deriveSwapParamsAtIndex(7);
    const walletAddress = client.getEvmAddress();
    const perSwapAddress = deriveEvmAddress(swapParams.secretKey);
    expect(perSwapAddress.toLowerCase()).not.toBe(walletAddress.toLowerCase());

    const post = vi.fn(
      async (_path: string, opts: { body: Record<string, unknown> }) => ({
        data: {
          id: "11111111-2222-3333-4444-555555555555",
          client_evm_address: opts.body.claiming_address,
          bolt11_invoice: "lnbc1...",
          target_amount: "1",
        },
        error: undefined,
        response: new Response(null, { status: 200 }),
      }),
    );
    const stored: unknown[] = [];
    const ctx = {
      apiClient: { POST: post } as unknown as CreateSwapContext["apiClient"],
      deriveSwapParams: async () => swapParams,
      evmAddress: walletAddress,
      storeSwap: async (...args: unknown[]) => {
        stored.push(args);
      },
    } as unknown as CreateSwapContext;

    await createLightningToEvmSwap(
      {
        targetAddress: "0x000000000000000000000000000000000000dEaD",
        tokenAddress: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
        evmChainId: 42161,
        sourceAmount: 20000,
      },
      ctx,
    );

    expect(post).toHaveBeenCalledTimes(1);
    const body = post.mock.calls[0][1].body;
    expect(body.claiming_address).toBe(perSwapAddress);
    expect(body.target_address).toBe(
      "0x000000000000000000000000000000000000dEaD",
    );
    expect(stored).toHaveLength(1);
  });
});
