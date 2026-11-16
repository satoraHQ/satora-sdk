import {
  type IndexerProvider,
  InMemoryContractRepository,
  InMemoryWalletRepository,
  type IWallet,
  type OperatorProvider,
} from "@arkade-os/sdk";
import { describe, expect, it, vi } from "vitest";
import { EscrowClient, type SwapClient } from "./escrow-client.js";

const LN_ADDRESS = "alice@example.com";
const VHTLC_ADDRESS = "tark1vhtlc";

function setup(sourceAmount = "1050") {
  const swap = {
    createLightningToArkadeSwap: vi.fn(),
    claimArkade: vi.fn(),
    createArkadeToLightningSwap: vi.fn().mockResolvedValue({
      response: {
        id: "swap-1",
        arkade_vhtlc_address: VHTLC_ADDRESS,
        source_amount: sourceAmount,
      },
      swapParams: {},
    }),
    getQuote: vi.fn().mockResolvedValue({
      net_source_amount: "999",
      net_target_amount: "950",
    }),
  };
  const wallet = { send: vi.fn().mockResolvedValue("fund-txid") };
  const client = EscrowClient.create({
    swap: swap as unknown as SwapClient,
    arkProvider: {} as OperatorProvider,
    indexerProvider: {} as IndexerProvider,
    contractRepository: new InMemoryContractRepository(),
    walletRepository: new InMemoryWalletRepository(),
  });
  return { swap, wallet: wallet as unknown as IWallet & typeof wallet, client };
}

describe("EscrowClient Lightning withdrawal", () => {
  it("quotes an Arkade→Lightning withdrawal from the source amount", async () => {
    const { swap, client } = setup();
    const quote = await (await client).quoteLightningWithdrawal(
      999,
      ` ${LN_ADDRESS} `,
    );
    expect(swap.getQuote).toHaveBeenCalledWith({
      sourceChain: "Arkade",
      sourceToken: "btc",
      targetChain: "Lightning",
      targetToken: "btc",
      sourceAmount: 999,
      lightningDestination: LN_ADDRESS,
    });
    expect(quote).toEqual({ recipientSats: 950, sourceSats: 999 });
  });

  it("creates the swap and funds the VHTLC with its source amount", async () => {
    const { swap, wallet, client } = setup("1050");
    const res = await (await client).withdrawToLightning({
      wallet,
      destination: LN_ADDRESS,
      amountSats: 1000,
    });
    expect(swap.createArkadeToLightningSwap).toHaveBeenCalledWith({
      lightningAddress: LN_ADDRESS,
      targetAmountSats: 1000,
    });
    expect(wallet.send).toHaveBeenCalledWith({
      address: VHTLC_ADDRESS,
      amount: 1050,
    });
    expect(res).toEqual({
      swapId: "swap-1",
      fundingTxid: "fund-txid",
      sourceAmountSats: 1050,
    });
  });

  it("routes a Lightning destination through withdraw()", async () => {
    const { wallet, client } = setup();
    const res = await (await client).withdraw({
      wallet,
      destination: LN_ADDRESS,
      amountSats: 1000,
    });
    expect(res).toEqual({
      method: "lightning",
      txid: "fund-txid",
      swapId: "swap-1",
      sourceAmountSats: 1050,
    });
  });

  it("rejects a Lightning address without an amount before creating a swap", async () => {
    const { swap, wallet, client } = setup();
    await expect(
      (await client).withdrawToLightning({ wallet, destination: LN_ADDRESS }),
    ).rejects.toThrow(/amountSats is required/);
    expect(swap.createArkadeToLightningSwap).not.toHaveBeenCalled();
    expect(wallet.send).not.toHaveBeenCalled();
  });
});
