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

  it("returns the mined hash and only warns when its receipt shows no SwapCreated", async () => {
    // The call this client sent mined with status success, so the HTLC exists
    // whatever a filtered receipt shows; failing would invite a second funding.
    serverAnswering({ swap: swapResponse(), permit2: permit2Params });
    const warn = vi.fn();
    const { client, storage } = await unfundedClient({ warn });
    const { txHash } = await client.fundSwap(
      SWAP_ID,
      fundingSigner({ transactionHash: SENT, logs: [] }),
    );
    expect(txHash).toBe(SENT);
    expect(warn).toHaveBeenCalledWith(
      expect.objectContaining({ event: "client.fundSwap.htlcLogNotFound" }),
    );
    expect(await storage.get(SWAP_ID)).toMatchObject({ evmFundTxid: SENT });
  });

  it("verifies a replacement through the mined call when the signer has no logs", async () => {
    // A repriced funding is the same call under a new hash; a cancel is not.
    for (const [replacement, sameCall] of [
      [MINED, true],
      [`0x${"cc".repeat(32)}`, false],
    ] as const) {
      serverAnswering({ swap: swapResponse(), permit2: permit2Params });
      const { client, storage } = await unfundedClient();
      let sent: { to: string; data: string } | undefined;
      const signer = signerWith({
        signTypedData: async () => `0x${"11".repeat(64)}1b`,
        sendTransaction: async (tx) => {
          sent = { to: tx.to, data: tx.data };
          return SENT;
        },
        waitForReceipt: async () => ({
          status: "success",
          blockNumber: 1n,
          transactionHash: replacement,
        }),
        getTransaction: async () =>
          sameCall
            ? {
                to: sent?.to ?? null,
                input: sent?.data ?? "0x",
                from: SIGNER_ADDRESS,
              }
            : { to: SIGNER_ADDRESS, input: "0x", from: SIGNER_ADDRESS },
      });
      if (sameCall) {
        expect((await client.fundSwap(SWAP_ID, signer)).txHash).toBe(
          replacement,
        );
        expect(await storage.get(SWAP_ID)).toMatchObject({
          evmFundTxid: replacement,
        });
      } else {
        await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
          /did not create the HTLC/,
        );
        expect(await storage.get(SWAP_ID)).toMatchObject({ evmFundTxid: SENT });
      }
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

describe("the server's delegation hint is bound to the address it describes", () => {
  afterEach(() => vi.unstubAllGlobals());

  const FOREIGN_DELEGATION = `0x${"ab".repeat(20)}`;
  const relay = {
    id: SWAP_ID,
    status: "clientfunded",
    tx_hash: `0x${"cd".repeat(32)}`,
    message: "",
  };

  async function permit2SignatureSent(fetchMock: ReturnType<typeof vi.fn>) {
    const call = fetchMock.mock.calls.find(([input]) =>
      (input instanceof Request ? input.url : String(input)).includes(
        "fund-gasless",
      ),
    );
    if (!call) throw new Error("relay was not called");
    const [input, init] = call as [RequestInfo | URL, RequestInit | undefined];
    const raw =
      typeof init?.body === "string"
        ? init.body
        : await (input as Request).text();
    return (JSON.parse(raw) as { permit2_signature: string }).permit2_signature;
  }

  it("ignores a hint for a different address and signs as a plain EOA", async () => {
    // A non-gasless-style swap: client_evm_address is the user's wallet,
    // not the key this SDK signs Permit2 with. The server probed THAT
    // address and found a foreign delegation.
    const fetchMock = serverAnswering({
      swap: swapResponse({ clientEvmAddress: SIGNER_ADDRESS }),
      permit2: { ...permit2Params, depositor_delegation: FOREIGN_DELEGATION },
      gasless: relay,
    });
    const storage = new InMemorySwapStorage();
    await storage.store(
      storedSwap({ evmFundTxid: undefined, evmCoordinatorAddress: undefined }),
    );
    const client = await Client.builder()
      .withSwapStorage(storage)
      .withMnemonic(MNEMONIC)
      .build();
    expect(client.getEvmAddress().toLowerCase()).not.toBe(
      SIGNER_ADDRESS.toLowerCase(),
    );

    await expect(client.fundSwapGasless(SWAP_ID)).resolves.toMatchObject({
      txHash: relay.tx_hash,
    });
    // 65 bytes: r || s || v, no Kernel envelope
    expect((await permit2SignatureSent(fetchMock)).length).toBe(2 + 130);
  });

  it("refuses a foreign delegation on the address it actually signs for", async () => {
    // The API client captures `fetch` at build time, so derive the SDK's
    // address from a throwaway client, stub, then build the one under test.
    const sdkAddress = (
      await Client.builder().withMnemonic(MNEMONIC).build()
    ).getEvmAddress();
    serverAnswering({
      swap: swapResponse({ clientEvmAddress: sdkAddress }),
      permit2: { ...permit2Params, depositor_delegation: FOREIGN_DELEGATION },
      gasless: relay,
    });
    const storage = new InMemorySwapStorage();
    await storage.store(
      storedSwap({ evmFundTxid: undefined, evmCoordinatorAddress: undefined }),
    );
    const client = await Client.builder()
      .withSwapStorage(storage)
      .withMnemonic(MNEMONIC)
      .build();

    await expect(client.fundSwapGasless(SWAP_ID)).rejects.toThrow(
      /delegated to 0xabab.*cannot sign for/,
    );
  });
});
