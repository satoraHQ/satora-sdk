import { encodeFunctionData, parseAbiItem } from "viem";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  encodeNativeExecuteAndCreate,
  encodeNativeRefundTo,
  NATIVE_TOKEN_ADDRESS,
} from "../src/evm/coordinator.js";
import type { EvmFundingTerms } from "../src/evm/funding-pins.js";
import { encodeHtlcNativeRefundCallData } from "../src/evm/htlc.js";
import type { EvmSigner, ReceiptLog, TxReceipt } from "../src/evm/wallet.js";
import { Client, InMemorySwapStorage } from "../src/index.js";
import type { StoredSwap } from "../src/storage/types.js";
import {
  COORDINATOR,
  fundingTerms,
  HASH_LOCK,
  HTLC,
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
} from "./evm-funding-fixture.js";

// HTLCNativeCoordinator / HTLCNative: the ERC20 signatures without `token`.
const executeAndCreateAbi = parseAbiItem([
  "function executeAndCreate((address target, uint256 value, bytes callData)[] calls, bytes32 preimageHash, uint256 amount, address claimAddress, uint256 timelock) payable",
]);
const nativeRefundToAbi = parseAbiItem(
  "function refundTo(bytes32 preimageHash, uint256 amount, address claimAddress, uint256 timelock)",
);
const nativeRefundAbi = parseAbiItem(
  "function refund(bytes32 preimageHash, uint256 amount, address claimAddress, uint256 timelock)",
);

const NATIVE_EXECUTE_AND_CREATE_SELECTOR = "0x81cf2ad5";
const NATIVE_REFUND_TO_SELECTOR = "0xa7b3be10";
const NATIVE_REFUND_SELECTOR = "0x35cd4ccb";

/** 0.0001 RBTC, in wei: what the swap below expects locked. */
const AMOUNT_WEI = 100_000_000_000_000n;

/**
 * {@link REAL_LOG} as `HTLCNative` emits it: the same layout with the asset
 * word zeroed (the lock is the chain's coin) and the amount in wei.
 */
function nativeLog(sender = SIGNER_ADDRESS): ReceiptLog {
  const log = logFrom(sender);
  const words = log.data.slice(2).match(/.{64}/g) ?? [];
  words[0] = "0".repeat(64);
  words[1] = AMOUNT_WEI.toString(16).padStart(64, "0");
  return { ...log, data: `0x${words.join("")}` };
}

function nativeSwapResponse(overrides: { fundTxid?: string | null } = {}) {
  return {
    ...(swapResponse({
      direction: "evm_to_lightning",
      clientEvmAddress: SIGNER_ADDRESS,
      fundTxid: overrides.fundTxid,
    }) as object),
    evm_chain_id: 30,
    evm_coordinator_address: NATIVE_TOKEN_ADDRESS,
    evm_htlc_kind: "native",
    evm_expected_sats: AMOUNT_WEI.toString(),
    source_amount: AMOUNT_WEI.toString(),
    source_token: {
      token_id: NATIVE_TOKEN_ADDRESS,
      symbol: "RBTC",
      chain: "30",
      name: "Rootstock BTC",
      decimals: 18,
    },
  } as unknown as StoredSwap["response"];
}

/** The funding terms this SDK records when it creates the swap above. */
function nativeTerms(overrides: Partial<EvmFundingTerms> = {}) {
  return fundingTerms({
    chainId: 30,
    coordinator: NATIVE_TOKEN_ADDRESS,
    htlcKind: "native",
    sourceToken: NATIVE_TOKEN_ADDRESS,
    sourceAmount: AMOUNT_WEI.toString(),
    lockToken: undefined,
    ...overrides,
  });
}

describe("native coordinator encoders", () => {
  it("executeAndCreate with no calls matches viem's ABI encoding", () => {
    const ours = encodeNativeExecuteAndCreate(COORDINATOR, {
      calls: [],
      preimageHash: HASH_LOCK,
      amount: AMOUNT_WEI,
      claimAddress: SERVER,
      timelock: TIMELOCK,
    });
    const ref = encodeFunctionData({
      abi: [executeAndCreateAbi],
      functionName: "executeAndCreate",
      args: [[], HASH_LOCK, AMOUNT_WEI, SERVER, BigInt(TIMELOCK)],
    });
    expect(ours.data).toBe(ref);
    expect(ours.data.startsWith(NATIVE_EXECUTE_AND_CREATE_SELECTOR)).toBe(true);
    expect(ours.to).toBe(COORDINATOR);
    // a plain lock sends exactly the amount
    expect(ours.value).toBe(AMOUNT_WEI);
  });

  it("executeAndCreate with calls encodes the array and adds their value", () => {
    const call = { target: HTLC, value: 5n, data: "0xabcdef" };
    const ours = encodeNativeExecuteAndCreate(COORDINATOR, {
      calls: [call],
      preimageHash: HASH_LOCK,
      amount: AMOUNT_WEI,
      claimAddress: SERVER,
      timelock: TIMELOCK,
    });
    const ref = encodeFunctionData({
      abi: [executeAndCreateAbi],
      functionName: "executeAndCreate",
      args: [
        [{ target: HTLC, value: 5n, callData: "0xabcdef" }],
        HASH_LOCK,
        AMOUNT_WEI,
        SERVER,
        BigInt(TIMELOCK),
      ],
    });
    expect(ours.data).toBe(ref);
    expect(ours.value).toBe(AMOUNT_WEI + 5n);
  });

  it("refundTo on the native coordinator matches viem's ABI encoding", () => {
    const ours = encodeNativeRefundTo(COORDINATOR, {
      preimageHash: HASH_LOCK,
      amount: AMOUNT_WEI,
      claimAddress: SERVER,
      timelock: TIMELOCK,
    });
    const ref = encodeFunctionData({
      abi: [nativeRefundToAbi],
      functionName: "refundTo",
      args: [HASH_LOCK, AMOUNT_WEI, SERVER, BigInt(TIMELOCK)],
    });
    expect(ours.data).toBe(ref);
    expect(ours.data.startsWith(NATIVE_REFUND_TO_SELECTOR)).toBe(true);
  });

  it("refund on HTLCNative matches viem's ABI encoding", () => {
    const ours = encodeHtlcNativeRefundCallData(HTLC, {
      preimageHash: HASH_LOCK,
      amount: AMOUNT_WEI,
      claimAddress: SERVER,
      timelock: TIMELOCK,
    });
    const ref = encodeFunctionData({
      abi: [nativeRefundAbi],
      functionName: "refund",
      args: [HASH_LOCK, AMOUNT_WEI, SERVER, BigInt(TIMELOCK)],
    });
    expect(ours.data).toBe(ref);
    expect(ours.data.startsWith(NATIVE_REFUND_SELECTOR)).toBe(true);
  });
});

describe("fundSwap on a native lock", () => {
  afterEach(() => vi.unstubAllGlobals());

  const SENT = `0x${"aa".repeat(32)}`;

  async function unfundedClient() {
    const storage = new InMemorySwapStorage();
    await storage.store(
      storedSwap({
        evmFundTxid: undefined,
        evmCoordinatorAddress: undefined,
        evmFundingTerms: nativeTerms(),
        response: nativeSwapResponse(),
      }),
    );
    const client = await Client.builder().withSwapStorage(storage).build();
    return { client, storage };
  }

  it("funds the HTLC directly and never asks for Permit2 params", async () => {
    const fetchMock = serverAnswering({ swap: nativeSwapResponse() });
    const { client, storage } = await unfundedClient();
    const sent: Array<{
      to: string;
      data: string;
      value?: bigint;
      type?: "legacy";
      gas?: bigint;
    }> = [];
    const simulated: Array<{ to: string; data: string; value?: bigint }> = [];
    const signer = signerWith({
      chainId: 30,
      getBalance: async () => AMOUNT_WEI * 2n,
      call: async (tx) => {
        simulated.push(tx);
        return "0x";
      },
      sendTransaction: async (tx) => {
        sent.push(tx);
        return SENT;
      },
      logs: [nativeLog()],
    });

    const { txHash } = await client.fundSwap(SWAP_ID, signer);

    expect(txHash).toBe(SENT);
    expect(sent).toHaveLength(1);
    expect(sent[0].to).toBe(HTLC);
    expect(sent[0].value).toBe(AMOUNT_WEI);
    // Rootstock rejects typed transactions
    expect(sent[0].type).toBe("legacy");
    expect(sent[0].data).toBe(
      encodeFunctionData({
        abi: [
          parseAbiItem(
            "function create(bytes32 preimageHash, address claimAddress, uint256 timelock) payable",
          ),
        ],
        functionName: "create",
        args: [HASH_LOCK, SERVER, BigInt(TIMELOCK)],
      }),
    );
    // simulated with the same value, so a short wallet fails before signing
    expect(simulated).toHaveLength(1);
    expect(simulated[0].value).toBe(AMOUNT_WEI);
    const stored = await storage.get(SWAP_ID);
    expect(stored?.evmFundTxid).toBe(SENT);
    // The wallet sends a native lock itself, through no coordinator.
    expect(stored?.evmCoordinatorAddress).toBeUndefined();
    for (const call of fetchMock.mock.calls) {
      expect(String(call[0])).not.toContain("swap-and-lock-calldata");
    }
  });

  it("refuses a wallet that cannot cover the lock before it signs", async () => {
    serverAnswering({ swap: nativeSwapResponse() });
    const { client } = await unfundedClient();
    const sendTransaction = vi.fn(async () => SENT);
    const signer = signerWith({
      chainId: 30,
      getBalance: async () => AMOUNT_WEI - 1n,
      sendTransaction,
    });

    await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      /Insufficient native balance/,
    );
    expect(sendTransaction).not.toHaveBeenCalled();
  });

  it("refuses a lock under another hash than the swap's before it signs", async () => {
    const foreign = `0x${"f0".repeat(32)}`;
    serverAnswering({ swap: { ...nativeSwapResponse(), hash_lock: foreign } });
    const storage = new InMemorySwapStorage();
    await storage.store(
      storedSwap({
        evmFundTxid: undefined,
        evmCoordinatorAddress: undefined,
        evmFundingTerms: nativeTerms(),
        response: { ...nativeSwapResponse(), hash_lock: foreign } as never,
      }),
    );
    const client = await Client.builder().withSwapStorage(storage).build();
    const sendTransaction = vi.fn(async () => SENT);
    const signer = signerWith({
      chainId: 30,
      getBalance: async () => AMOUNT_WEI * 2n,
      sendTransaction,
    });

    await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      "not this swap's",
    );
    expect(sendTransaction).not.toHaveBeenCalled();
  });

  it("refuses to send the coin to an HTLC the swap was not created with", async () => {
    const attacker = `0x${"a7".repeat(20)}`;
    // A server refresh rewrote the stored response; the terms recorded at
    // creation still name the real HTLC.
    const named = { ...nativeSwapResponse(), evm_htlc_address: attacker };
    serverAnswering({ swap: named });
    const { client, storage } = await unfundedClient();
    await storage.update(SWAP_ID, named as never);
    const sendTransaction = vi.fn(async () => SENT);
    const signer = signerWith({
      chainId: 30,
      getBalance: async () => AMOUNT_WEI * 2n,
      sendTransaction,
    });

    await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      "it was created with",
    );
    expect(sendTransaction).not.toHaveBeenCalled();
  });

  it("refuses a signer on another chain before it signs", async () => {
    serverAnswering({ swap: nativeSwapResponse() });
    const { client } = await unfundedClient();
    const sendTransaction = vi.fn(async () => SENT);
    const signer = signerWith({ chainId: 137, sendTransaction });

    await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      /chain 137 .* chain 30/,
    );
    expect(sendTransaction).not.toHaveBeenCalled();
  });
});

describe("fundSwap retries a native lock that was never funded", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  const FIRST = `0x${"a1".repeat(32)}`;
  const SECOND = `0x${"a2".repeat(32)}`;

  async function clientWith(stored: Partial<StoredSwap> = {}) {
    serverAnswering({ swap: nativeSwapResponse() });
    const storage = new InMemorySwapStorage();
    await storage.store(
      storedSwap({
        evmFundTxid: undefined,
        evmCoordinatorAddress: undefined,
        evmFundingTerms: nativeTerms(),
        response: nativeSwapResponse(),
        ...stored,
      }),
    );
    const client = await Client.builder().withSwapStorage(storage).build();
    return { client, storage };
  }

  /** A wallet sending FIRST, then SECOND; `receipts` answers each wait. */
  function wallet(
    receipts: Record<string, () => Promise<TxReceipt>>,
    unknownHashes: string[] = [],
  ) {
    const sent = [FIRST, SECOND];
    const sendTransaction = vi.fn(async () => sent.shift() as string);
    const mined = (hash: string): Promise<TxReceipt> =>
      Promise.resolve({
        status: "success",
        blockNumber: 1n,
        transactionHash: hash,
        logs: [nativeLog()],
      });
    const signer = signerWith({
      chainId: 30,
      getBalance: async () => AMOUNT_WEI * 2n,
      sendTransaction,
      unknownHashes,
    });
    signer.waitForReceipt = (hash) => (receipts[hash] ?? (() => mined(hash)))();
    return { signer, sendTransaction };
  }

  it("funds again after the first native lock reverted", async () => {
    const { client, storage } = await clientWith();
    const { signer, sendTransaction } = wallet({
      [FIRST]: async () => ({
        status: "reverted",
        blockNumber: 1n,
        transactionHash: FIRST,
        logs: [],
      }),
    });

    await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      /Funding transaction failed/,
    );
    const { txHash } = await client.fundSwap(SWAP_ID, signer);

    expect(txHash).toBe(SECOND);
    expect(sendTransaction).toHaveBeenCalledTimes(2);
    const stored = await storage.get(SWAP_ID);
    expect(stored?.evmFundTxid).toBe(SECOND);
    // The wallet sent the lock itself; it is no coordinator.
    expect(stored?.evmCoordinatorAddress).toBeUndefined();
  });

  it("funds again after the wallet cancelled the first native lock", async () => {
    const { client } = await clientWith();
    const cancel = `0x${"cc".repeat(32)}`;
    // A cancel mines in the same nonce, with no logs.
    const { signer, sendTransaction } = wallet({
      [FIRST]: async () => ({
        status: "success",
        blockNumber: 1n,
        transactionHash: cancel,
        logs: [],
      }),
    });

    await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      /did not create the HTLC/,
    );
    const { txHash } = await client.fundSwap(SWAP_ID, signer);

    expect(txHash).toBe(SECOND);
    expect(sendTransaction).toHaveBeenCalledTimes(2);
  });

  it("funds again over an older SDK's record of a reverted lock", async () => {
    // Older SDKs recorded the wallet as the coordinator of a native lock.
    const { client, storage } = await clientWith({
      evmFundTxid: FIRST,
      evmCoordinatorAddress: SIGNER_ADDRESS,
    });
    const { signer, sendTransaction } = wallet({
      [FIRST]: async () => ({
        status: "reverted",
        blockNumber: 1n,
        transactionHash: FIRST,
        logs: [],
      }),
    });
    // The first wallet tx is never sent here: FIRST is the recorded one.
    sendTransaction.mockImplementationOnce(async () => SECOND);

    const { txHash } = await client.fundSwap(SWAP_ID, signer);

    expect(txHash).toBe(SECOND);
    expect((await storage.get(SWAP_ID))?.evmFundTxid).toBe(SECOND);
  });

  it("refuses to fund again while the first native lock may still land", async () => {
    vi.useFakeTimers();
    const { client } = await clientWith({
      evmFundTxid: FIRST,
      evmCoordinatorAddress: SIGNER_ADDRESS,
    });
    const { signer, sendTransaction } = wallet({
      [FIRST]: () => new Promise<TxReceipt>(() => {}),
    });

    const refused = expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      /is not mined yet; refusing to fund it again/,
    );
    await vi.advanceTimersByTimeAsync(60_000);
    await refused;
    expect(sendTransaction).not.toHaveBeenCalled();
  });

  it("refuses to fund again a lock the node does not know", async () => {
    const { client } = await clientWith({ evmFundTxid: FIRST });
    const { signer, sendTransaction } = wallet({}, [FIRST]);

    await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      /may still land; refusing to fund it again/,
    );
    expect(sendTransaction).not.toHaveBeenCalled();
  });

  it("refuses to fund again a lock that mined", async () => {
    const { client } = await clientWith({ evmFundTxid: FIRST });
    const { signer, sendTransaction } = wallet({});

    await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      /refusing to fund it again/,
    );
    expect(sendTransaction).not.toHaveBeenCalled();
  });

  /** The wallet's first lock replaced by `replacement`; SECOND would follow. */
  async function replacedBy(
    replacement: Partial<TxReceipt>,
    getTransaction?: EvmSigner["getTransaction"],
  ) {
    const { client, storage } = await clientWith();
    const { signer, sendTransaction } = wallet({
      [FIRST]: async () => ({
        status: "success",
        blockNumber: 1n,
        transactionHash: REPLACEMENT,
        logs: [],
        ...replacement,
      }),
    });
    if (getTransaction) signer.getTransaction = getTransaction;
    await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      /did not create the HTLC/,
    );
    return { client, storage, signer, sendTransaction };
  }

  const REPLACEMENT = `0x${"cc".repeat(32)}`;

  it("keeps the record of a replacement whose receipt has no logs", async () => {
    // The replacement is another call: without logs nothing shows whether
    // it locked the coin, say under another timelock.
    const other = async () => ({
      to: HTLC,
      input: "0xdead",
      from: SIGNER_ADDRESS,
    });
    const { client, storage, signer, sendTransaction } = await replacedBy(
      { logs: undefined },
      other,
    );

    expect((await storage.get(SWAP_ID))?.evmFundTxid).toBe(FIRST);
    await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      `was replaced by ${REPLACEMENT}, which may have locked the coin`,
    );
    expect(sendTransaction).toHaveBeenCalledTimes(1);
  });

  it("keeps the record when the replacement cannot be looked up", async () => {
    const { client, storage, signer, sendTransaction } = await replacedBy(
      { logs: undefined },
      async (hash) => {
        if (hash === REPLACEMENT) throw new Error("rate limited");
        return { to: HTLC, input: "0x", from: SIGNER_ADDRESS };
      },
    );

    expect((await storage.get(SWAP_ID))?.evmFundTxid).toBe(FIRST);
    await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      /refusing to fund it again/,
    );
    expect(sendTransaction).toHaveBeenCalledTimes(1);
  });

  it("keeps the record of a replacement that locked under other terms", async () => {
    // The same hash locked for another claimer: not this swap's lock, but
    // not proof that the coin is still in the wallet either.
    const { client, storage, signer, sendTransaction } = await replacedBy({
      logs: [
        { ...nativeLog(), topics: logFrom(SIGNER_ADDRESS, COORDINATOR).topics },
      ],
    });

    expect((await storage.get(SWAP_ID))?.evmFundTxid).toBe(FIRST);
    await expect(client.fundSwap(SWAP_ID, signer)).rejects.toThrow(
      /may have locked the coin/,
    );
    expect(sendTransaction).toHaveBeenCalledTimes(1);
  });

  it("drops the record of a replacement whose logs show no lock", async () => {
    const { client, storage, signer, sendTransaction } = await replacedBy({
      logs: [],
    });

    expect((await storage.get(SWAP_ID))?.evmFundTxid).toBeUndefined();
    const { txHash } = await client.fundSwap(SWAP_ID, signer);
    expect(txHash).toBe(SECOND);
    expect(sendTransaction).toHaveBeenCalledTimes(2);
  });

  it("still refuses a coordinator other than the one a native swap was created with", async () => {
    // A legacy Rootstock swap funded through a coordinator: its record is
    // a coordinator, not the wallet, and is checked as one.
    const attacker = `0x${"a7".repeat(20)}`;
    const { client } = await clientWith({
      evmFundingTerms: nativeTerms({ coordinator: COORDINATOR }),
      evmCoordinatorAddress: attacker,
    });

    await expect(client.getEvmFundingExpectation(SWAP_ID)).rejects.toThrow(
      `was funded through coordinator ${attacker}`,
    );
  });
});

describe("the coordinator check on an ERC-20 swap", () => {
  afterEach(() => vi.unstubAllGlobals());

  const erc20 = swapResponse({
    direction: "evm_to_lightning",
    clientEvmAddress: SIGNER_ADDRESS,
  }) as object;

  it.each([
    ["an attacker's coordinator", `0x${"a7".repeat(20)}`, erc20],
    // The native exemption for the wallet does not reach ERC-20 swaps.
    ["the depositor's own address", SIGNER_ADDRESS, erc20],
    // Nor does a refreshed response that calls the swap native: the
    // exemption reads the terms recorded at creation.
    [
      "the depositor under a response rewritten as native",
      SIGNER_ADDRESS,
      {
        ...erc20,
        evm_htlc_kind: "native",
        evm_coordinator_address: NATIVE_TOKEN_ADDRESS,
      },
    ],
  ])("refuses %s on record", async (_, recorded, response) => {
    const storage = new InMemorySwapStorage();
    await storage.store(
      storedSwap({
        evmCoordinatorAddress: recorded,
        response: response as StoredSwap["response"],
      }),
    );
    const client = await Client.builder().withSwapStorage(storage).build();

    await expect(client.getEvmFundingExpectation(SWAP_ID)).rejects.toThrow(
      `was funded through coordinator ${recorded}, not the ${COORDINATOR}`,
    );
  });
});

describe("refundSwap on a native lock", () => {
  afterEach(() => vi.unstubAllGlobals());

  async function clientWith(stored: StoredSwap) {
    const storage = new InMemorySwapStorage();
    await storage.store(stored);
    return Client.builder().withSwapStorage(storage).build();
  }

  it("builds the native coordinator's refundTo from a zero-asset SwapCreated log", async () => {
    const fetchMock = serverGone();
    const client = await clientWith(
      storedSwap({ response: nativeSwapResponse() }),
    );

    const result = await client.refundSwap(SWAP_ID, {
      signer: signerWith({ chainId: 30, logs: [nativeLog(COORDINATOR)] }),
    });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.to).toBe(COORDINATOR);
    expect(result.evmRefundData?.data).toBe(
      encodeNativeRefundTo(COORDINATOR, {
        preimageHash: HASH_LOCK,
        amount: AMOUNT_WEI,
        claimAddress: SERVER,
        timelock: TIMELOCK,
      }).data,
    );
    expect(result.evmRefundData?.recipient).toBe(SIGNER_ADDRESS);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("refunds a native lock the signer made straight on HTLCNative", async () => {
    serverGone();
    const client = await clientWith(
      storedSwap({ response: nativeSwapResponse() }),
    );

    const result = await client.refundSwap(SWAP_ID, {
      signer: signerWith({ chainId: 30, logs: [nativeLog(SIGNER_ADDRESS)] }),
    });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.to).toBe(HTLC);
    expect(result.evmRefundData?.data.startsWith(NATIVE_REFUND_SELECTOR)).toBe(
      true,
    );
  });
});
