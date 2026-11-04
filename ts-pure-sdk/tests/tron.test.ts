import { afterEach, describe, expect, it, vi } from "vitest";
import { Client } from "../src/client.js";
import { InMemorySwapStorage } from "../src/storage/index.js";
import type { StoredSwap } from "../src/storage/types.js";
import {
  Asset,
  getBridgeTargetChain,
  getUsdt0BridgeTokens,
  isBridgeOnlyChain,
  isEvmToken,
} from "../src/tokens.js";
import {
  getMessageStatus,
  trackMessage,
} from "../src/usdt0-bridge/tracking.js";
import {
  isValidTronAddress,
  tronAddressToBytes32,
} from "../src/usdt0-bridge/tron.js";

const ADDRESS = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
afterEach(() => vi.unstubAllGlobals());

describe("Tron payouts", () => {
  it("quotes the Arbitrum USDT0 leg with a Tron destination and preserves the mesh fee", async () => {
    const requests: URL[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (request: Request) => {
        requests.push(new URL(request.url));
        return Response.json({
          exchange_rate: "100000",
          network_fee: 100,
          gasless_network_fee: 10,
          protocol_fee: 0,
          protocol_fee_rate: 0,
          min_amount: 1,
          max_amount: 1_000_000,
          source_amount: "100000",
          target_amount: "100000000",
          net_source_amount: "100000",
          net_target_amount: "99860000",
          bridge_fee: 30000,
        });
      }),
    );
    const client = await Client.builder()
      .withBaseUrl("https://example.test")
      .build();
    const quote = await client.getQuote({
      sourceChain: "Lightning",
      sourceToken: "btc",
      targetChain: "Tron",
      targetToken: ADDRESS,
      sourceAmount: 100_000,
    });
    expect(requests).toHaveLength(1);
    expect(requests[0].pathname).toBe("/quote");
    expect(requests[0].searchParams.get("target_chain")).toBe("42161");
    expect(requests[0].searchParams.get("target_token")).toBe(
      "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9",
    );
    expect(requests[0].searchParams.get("bridge_target_chain")).toBe("Tron");
    expect(quote.bridge_fee).toBe(30000);
  });
  it("selects the Tron message from a transaction with several messages", async () => {
    const message = (eid: number, guid: string) => ({
      pathway: {
        srcEid: 30110,
        dstEid: eid,
        sender: { chain: "arbitrum" },
        receiver: { chain: "tron" },
      },
      source: { tx: { txHash: "0xsource" } },
      destination: { tx: { txHash: "tron-destination-tx" } },
      status: { name: "DELIVERED", message: "" },
      guid,
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        Response.json({
          data: [message(30111, "unrelated"), message(30420, "tron")],
        }),
      ),
    );
    await expect(
      trackMessage({ txHash: "0xsource", destinationEid: 30420 }),
    ).resolves.toMatchObject({ guid: "tron", dstEid: 30420 });
    await expect(
      getMessageStatus("0xsource", undefined, 30420),
    ).resolves.toMatchObject({ guid: "tron" });
    await expect(
      getMessageStatus("0xsource", undefined, 99999),
    ).resolves.toBeNull();
  });
  it("validates the checksum and strips the network prefix for TVM", () => {
    expect(tronAddressToBytes32(ADDRESS)).toBe(
      "0x000000000000000000000000a614f803b6fd780986a42c78ec9c7f77e6ded13c",
    );
    expect(isValidTronAddress(`${ADDRESS.slice(0, -1)}u`)).toBe(false);
    expect(isValidTronAddress("T9yD14Nj9j7xAB4dbGeiX9h8unkKHxuWwb")).toBe(
      false,
    );
    expect(
      isValidTronAddress("0xa614f803b6fd780986a42c78ec9c7f77e6ded13c"),
    ).toBe(false);
  });

  it("lists native USDT as a bridge-only target with no EVM chain id", () => {
    const token = getUsdt0BridgeTokens().find((t) => t.chain === "Tron");
    expect(token).toMatchObject({
      token_id: ADDRESS,
      decimals: 6,
      symbol: "USDT",
    });
    expect(Asset.USDT_TRON).toEqual({ chain: "Tron", tokenId: ADDRESS });
    expect(isBridgeOnlyChain("Tron")).toBe(true);
    expect(isEvmToken("Tron")).toBe(false);
    expect(
      getBridgeTargetChain(token as Parameters<typeof getBridgeTargetChain>[0]),
    ).toBe("Tron");
  });

  it.each([
    [Asset.BTC_LIGHTNING, "createLightningToEvmSwap"],
    [Asset.BTC_ONCHAIN, "createBitcoinToEvmSwap"],
    [Asset.BTC_ARKADE, "createArkadeToEvmSwapGeneric"],
  ] as const)("routes %j through Arbitrum and pins the native recipient", async (source, method) => {
    const client = await Client.builder().build();
    const create = vi.spyOn(client, method).mockResolvedValue({} as never);
    await client.createSwap({
      source,
      target: Asset.USDT_TRON,
      sourceAmount: 100_000,
      targetAddress: ADDRESS,
    });
    expect(create).toHaveBeenCalledWith(
      expect.objectContaining({
        evmChainId: 42161,
        tokenAddress: "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9",
        targetAddress: ADDRESS,
        bridgeParams: expect.objectContaining({
          targetChain: "Tron",
          targetTokenAddress: ADDRESS,
          recipient: ADDRESS,
        }),
      }),
    );
  });

  it("rejects Tron deposits and malformed payouts before creating a swap", async () => {
    const client = await Client.builder().build();
    await expect(
      client.createSwap({
        source: Asset.USDT_TRON,
        target: Asset.BTC_LIGHTNING,
        sourceAmount: 10,
        targetAddress: "unused",
      }),
    ).rejects.toThrow("payout-only");
    await expect(
      client.createSwap({
        source: Asset.BTC_LIGHTNING,
        target: Asset.USDT_TRON,
        sourceAmount: 10,
        targetAddress: "0x1234",
      }),
    ).rejects.toThrow("Invalid Tron payout address");
  });

  it("uses the EVM claim wallet while recovering the Tron recipient from server metadata", async () => {
    const storage = new InMemorySwapStorage();
    const client = await Client.builder().withSwapStorage(storage).build();
    await storage.store({
      version: 2,
      swapId: "tron-swap",
      keyIndex: 0,
      preimage: "11".repeat(32),
      publicKey: "",
      secretKey: "",
      preimageHash: "",
      storedAt: 0,
      updatedAt: 0,
      response: {
        id: "tron-swap",
        direction: "lightning_to_evm",
        bridge_target_chain: "Tron",
        client_evm_address: "0x1111111111111111111111111111111111111111",
        target_evm_address: ADDRESS,
      },
    } as StoredSwap);
    const claim = vi
      .spyOn(client, "claimViaGasless")
      .mockResolvedValue({ txHash: "0x123", message: "submitted" } as never);
    await client.claim("tron-swap");
    expect(claim).toHaveBeenCalledWith(
      "tron-swap",
      "0x1111111111111111111111111111111111111111",
      {
        bridgeRecipient: ADDRESS,
        bridgeRecipientWallet: undefined,
      },
    );
  });
});
