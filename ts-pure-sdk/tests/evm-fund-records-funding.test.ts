import { afterEach, describe, expect, it, vi } from "vitest";
import { Client, InMemorySwapStorage } from "../src/index.js";
import {
  COORDINATOR,
  FUND_TXID,
  HASH_LOCK,
  logFrom,
  REAL_LOG,
  SERVER,
  SIGNER_ADDRESS,
  SWAP_ID,
  serverAnswering,
  serverGone,
  signerWith,
  storedSwap,
  swapResponse,
  TIMELOCK,
  USDC,
  WBTC,
} from "./evm-funding-fixture.js";

const permit2Params = {
  coordinator_address: COORDINATOR,
  permit2_address: "0x000000000022d473030f116ddee9f6b43ac78ba3",
  source_token_address: USDC,
  source_amount: "15658166",
  lock_token_address: WBTC,
  preimage_hash: HASH_LOCK,
  claim_address: SERVER,
  timelock: TIMELOCK,
  calls: [],
  calls_hash: `0x${"00".repeat(32)}`,
};

// The hash the wallet returned from sendTransaction, and the one that mined
// after the wallet repriced it.
const SENT = `0x${"aa".repeat(32)}`;
const MINED = FUND_TXID;

const MNEMONIC =
  "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

describe("fundSwap records the funding on the stored swap", () => {
  afterEach(() => vi.unstubAllGlobals());

  async function unfundedClient(
    options: { warn?: (record: unknown) => void } = {},
  ) {
    const storage = new InMemorySwapStorage();
    await storage.store(
      storedSwap({ evmFundTxid: undefined, evmCoordinatorAddress: undefined }),
    );
    let builder = Client.builder().withSwapStorage(storage);
    if (options.warn) {
      builder = builder.withLogger({ warn: options.warn }).withLogLevel("warn");
    }
    return { client: await builder.build(), storage };
  }

  function fundingSigner(receipt: {
    transactionHash: string;
    logs?: (typeof REAL_LOG)[];
  }) {
    return signerWith({
      signTypedData: async () => `0x${"11".repeat(64)}1b`,
      sendTransaction: async () => SENT,
      waitForReceipt: async () => ({
        status: "success",
        blockNumber: 1n,
        ...receipt,
      }),
    });
  }

  it("records the mined tx and the coordinator, which survive a server refresh", async () => {
    serverAnswering({ swap: swapResponse(), permit2: permit2Params });
    const { client, storage } = await unfundedClient();
    const signer = fundingSigner({ transactionHash: MINED, logs: [REAL_LOG] });

    const { txHash } = await client.fundSwap(SWAP_ID, signer);

    expect(txHash).toBe(MINED);
    expect(await storage.get(SWAP_ID)).toMatchObject({
      evmFundTxid: MINED,
      evmCoordinatorAddress: COORDINATOR,
    });

    // The server has not indexed the funding yet; refreshing must not
    // forget it, and the refund must then need no server at all.
    await client.getSwap(SWAP_ID, { updateStorage: true });
    expect(await storage.get(SWAP_ID)).toMatchObject({
      evmFundTxid: MINED,
      evmCoordinatorAddress: COORDINATOR,
      response: { evm_fund_txid: null },
    });

    // The SDK's API client binds fetch when it is built, so the refund runs
    // on a client built after the server is gone.
    const fetchMock = serverGone();
    const offline = await Client.builder().withSwapStorage(storage).build();
    const refund = await offline.refundSwap(SWAP_ID, { signer });
    expect(refund.success).toBe(true);
    expect(refund.evmRefundData?.to).toBe(COORDINATOR);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("rejects a replacement that did not create the HTLC", async () => {
    // A cancel mines with status success and no logs; the sent hash stays
    // recorded so a later refund can still find the real funding.
    serverAnswering({ swap: swapResponse(), permit2: permit2Params });
    const { client, storage } = await unfundedClient();
    const cancel = `0x${"cc".repeat(32)}`;
    const signer = fundingSigner({ transactionHash: cancel, logs: [] });

    await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      /did not create the HTLC/,
    );
    expect(await storage.get(SWAP_ID)).toMatchObject({
      evmFundTxid: SENT,
      evmCoordinatorAddress: COORDINATOR,
    });
  });

  it("rejects a replacement whose SwapCreated is not this swap's", async () => {
    // Same hash lock, but emitted by another contract or created by another
    // sender: neither is the HTLC the coordinator locks for this swap.
    for (const decoy of [
      { ...REAL_LOG, address: USDC },
      logFrom(SIGNER_ADDRESS),
    ]) {
      serverAnswering({ swap: swapResponse(), permit2: permit2Params });
      const { client, storage } = await unfundedClient();
      const signer = fundingSigner({ transactionHash: MINED, logs: [decoy] });
      await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
        /did not create the HTLC/,
      );
      expect(await storage.get(SWAP_ID)).toMatchObject({ evmFundTxid: SENT });
    }
  });

  it("warns when the signer's receipt carries no logs", async () => {
    serverAnswering({ swap: swapResponse(), permit2: permit2Params });
    const warn = vi.fn();
    const { client } = await unfundedClient({ warn });

    await client.fundSwap(SWAP_ID, fundingSigner({ transactionHash: SENT }));

    expect(warn).toHaveBeenCalledWith(
      expect.objectContaining({ event: "client.fundSwap.receiptWithoutLogs" }),
    );
  });
});

describe("fundSwapGasless records the funding on the stored swap", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("records the relay's tx and the coordinator", async () => {
    const relayTx = `0x${"cd".repeat(32)}`;
    serverAnswering({
      swap: swapResponse(),
      permit2: permit2Params,
      gasless: {
        id: SWAP_ID,
        status: "clientfunded",
        tx_hash: relayTx,
        message: "",
      },
    });
    const storage = new InMemorySwapStorage();
    await storage.store(
      storedSwap({ evmFundTxid: undefined, evmCoordinatorAddress: undefined }),
    );
    const client = await Client.builder()
      .withSwapStorage(storage)
      .withMnemonic(MNEMONIC)
      .build();

    const { txHash } = await client.fundSwapGasless(SWAP_ID);

    expect(txHash).toBe(relayTx);
    expect(await storage.get(SWAP_ID)).toMatchObject({
      evmFundTxid: relayTx,
      evmCoordinatorAddress: COORDINATOR,
    });
  });
});
