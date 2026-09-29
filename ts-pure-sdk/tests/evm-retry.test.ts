import { afterEach, describe, expect, it, vi } from "vitest";
import type { EvmSigner } from "../src/evm/wallet.js";
import { Client, InMemorySwapStorage } from "../src/index.js";
import { SWAP_ID, storedSwap, swapResponse } from "./evm-funding-fixture.js";

const WALLET = "0x90f79bf6eb2c4f870365e785982e1f101e93b906";
const ARBITRUM_TBTC = "0x6c84a8f1c29108F47a79964b5Fe888D4f4D0dE40";
const USER_OP = `0x${"0e".repeat(32)}` as const;
const REFUND_TX = `0x${"5e".repeat(32)}` as const;
const NEW_SWAP_ID = "3d41d067-cc82-4ddc-9f4e-e49a82b65a33";
const INVOICE =
  "lnbc200u1pjexample0000000000000000000000000000000000000000000000000";

/** A client holding a refunded Arbitrum swap of `direction`. */
async function clientWith(
  options: {
    depositorOf?: (sdkAddress: string) => string;
    direction?: "evm_to_bitcoin" | "evm_to_arkade" | "evm_to_lightning";
    chainId?: number;
  } = {},
) {
  const storage = new InMemorySwapStorage();
  const client = await Client.builder()
    .withSwapStorage(storage)
    .withAa({ bundlerUrl: "http://bundler.test" })
    .build();
  const depositor = (options.depositorOf ?? ((sdk) => sdk))(
    client.getEvmAddress(),
  );
  const response = {
    ...swapResponse({
      direction: options.direction,
      clientEvmAddress: depositor,
      gasless: true,
    }),
    evm_chain_id: options.chainId ?? 42161,
    status: "clientrefunded",
  } as ReturnType<typeof swapResponse>;
  await storage.store(storedSwap({ response }));
  return { client, response };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("Client.refundEvmSponsored", () => {
  function refundable(client: Client, timelockExpired = true) {
    vi.spyOn(client, "refundSwap").mockResolvedValue({
      success: true,
      message: "",
      evmRefundData: {
        to: "0x3a4cbee89a62c45d57c774527f931f0b67212c28",
        data: "0x29240837",
        timelockExpired,
        timelockExpiry: 1_800_000_000,
      },
    } as never);
    const sendUserOperation = vi.fn().mockResolvedValue(USER_OP);
    const createSmartAccountClient = vi
      .spyOn(client.cctpInbound, "createSmartAccountClient")
      .mockImplementation(
        async ({ signer }) =>
          ({
            accountAddress: signer.address,
            client: {
              sendUserOperation,
              waitForUserOperationReceipt: async () => ({
                receipt: { transactionHash: REFUND_TX },
              }),
            },
          }) as never,
      );
    return { sendUserOperation, createSmartAccountClient };
  }

  it("sends the refund as a UserOp from the SDK key's account", async () => {
    const { client } = await clientWith();
    const { sendUserOperation, createSmartAccountClient } = refundable(client);

    const result = await client.refundEvmSponsored(SWAP_ID);

    expect(result).toEqual({ txHash: REFUND_TX, userOpHash: USER_OP });
    const owner = createSmartAccountClient.mock.calls[0][0].signer;
    expect(owner.address.toLowerCase()).toBe(
      client.getEvmAddress().toLowerCase(),
    );
    expect(sendUserOperation).toHaveBeenCalledWith({
      calls: [
        {
          to: "0x3a4cbee89a62c45d57c774527f931f0b67212c28",
          data: "0x29240837",
          value: 0n,
        },
      ],
    });
  });

  it("refuses a wallet-funded swap, whose wallet refunds itself", async () => {
    const { client } = await clientWith({ depositorOf: () => WALLET });
    const { sendUserOperation } = refundable(client);

    await expect(client.refundEvmSponsored(SWAP_ID)).rejects.toThrow(
      /neither this client's key/,
    );
    expect(sendUserOperation).not.toHaveBeenCalled();
  });

  it("refuses a swap off Arbitrum", async () => {
    const { client } = await clientWith({ chainId: 137 });
    refundable(client);

    await expect(client.refundEvmSponsored(SWAP_ID)).rejects.toThrow(
      /Arbitrum only/,
    );
  });

  it("refuses before the timelock", async () => {
    const { client } = await clientWith();
    const { sendUserOperation } = refundable(client, false);

    await expect(client.refundEvmSponsored(SWAP_ID)).rejects.toThrow(
      /timelock has not expired/,
    );
    expect(sendUserOperation).not.toHaveBeenCalled();
  });
});

describe("Client.continueRefundedEvmSwap", () => {
  function continuable(client: Client, oldSwap: unknown) {
    vi.spyOn(client, "getRefundedEvmSwapContinuation").mockResolvedValue({
      eligible: true,
      smartAccountAddress: client.getEvmAddress(),
      token: {
        chain: "42161" as never,
        address: ARBITRUM_TBTC,
        symbol: "tBTC",
        decimals: 18,
      },
      amount: "200000000000000",
      oldSwap: oldSwap as never,
    });
    const created = { response: { id: NEW_SWAP_ID } } as never;
    const spies = {
      toBitcoin: vi
        .spyOn(client, "createEvmToBitcoinSwap")
        .mockResolvedValue(created),
      toArkade: vi
        .spyOn(client, "createEvmToArkadeSwapGeneric")
        .mockResolvedValue(created),
      toLightning: vi
        .spyOn(client, "createEvmToLightningSwap")
        .mockResolvedValue(created),
      submit: vi
        .spyOn(client.cctpInbound, "submitBalanceUserOp")
        .mockResolvedValue({
          userOpHash: USER_OP,
          transactionHash: REFUND_TX,
          smartAccountAddress: client.getEvmAddress(),
        } as never),
    };
    return spies;
  }

  it("swaps again to the old swap's payout type, funded by the SDK key", async () => {
    const { client, response } = await clientWith();
    const spies = continuable(client, response);

    const result = await client.continueRefundedEvmSwap(SWAP_ID, {
      targetAddress: "bc1qexample",
    });

    expect(spies.toBitcoin).toHaveBeenCalledWith(
      expect.objectContaining({
        targetAddress: "bc1qexample",
        tokenAddress: ARBITRUM_TBTC,
        evmChainId: 42161,
        sourceAmount: 200000000000000n,
        gasless: true,
      }),
    );
    const submitted = spies.submit.mock.calls[0][0];
    expect(submitted.swapId).toBe(NEW_SWAP_ID);
    expect(submitted.expectedMaxSourceAmount).toBe(200000000000000n);
    expect(submitted.signer.address.toLowerCase()).toBe(
      client.getEvmAddress().toLowerCase(),
    );
    expect(result.newSwapId).toBe(NEW_SWAP_ID);
  });

  it("retries a Bitcoin swap to a Lightning invoice", async () => {
    const { client, response } = await clientWith();
    const spies = continuable(client, response);

    await client.continueRefundedEvmSwap(SWAP_ID, {
      targetAddress: `lightning:${INVOICE}`,
      target: "lightning",
    });

    // The invoice pins the payout, so no source amount goes with it.
    const options = spies.toLightning.mock.calls[0][0];
    expect(options).toMatchObject({ lightningInvoice: INVOICE });
    expect(options).not.toHaveProperty("sourceAmount");
    expect(spies.toBitcoin).not.toHaveBeenCalled();
  });

  it("pays a Lightning address the whole refund", async () => {
    const { client, response } = await clientWith({
      direction: "evm_to_lightning",
    });
    const spies = continuable(client, response);

    await client.continueRefundedEvmSwap(SWAP_ID, {
      targetAddress: "satoshi@example.com",
    });

    expect(spies.toLightning).toHaveBeenCalledWith(
      expect.objectContaining({
        lightningAddress: "satoshi@example.com",
        sourceAmount: 200000000000000n,
      }),
    );
  });

  it("funds with the given signer for an account it owns", async () => {
    const { client, response } = await clientWith();
    const spies = continuable(client, response);
    const wallet = { address: WALLET } as EvmSigner;

    await client.continueRefundedEvmSwap(SWAP_ID, {
      targetAddress: "bc1qexample",
      signer: wallet,
    });

    expect(spies.submit.mock.calls[0][0].signer).toBe(wallet);
  });

  it("rejects a Lightning target that is not a Lightning payee", async () => {
    const { client, response } = await clientWith();
    const spies = continuable(client, response);

    await expect(
      client.continueRefundedEvmSwap(SWAP_ID, {
        targetAddress: "bc1qexample",
        target: "lightning",
      }),
    ).rejects.toThrow(/not a Lightning invoice/);
    expect(spies.submit).not.toHaveBeenCalled();
  });
});
