import { encodeFunctionData, parseAbiItem } from "viem";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  encodeNativeExecuteAndCreate,
  encodeNativeRefundTo,
  NATIVE_TOKEN_ADDRESS,
} from "../src/evm/coordinator.js";
import { encodeHtlcNativeRefundCallData } from "../src/evm/htlc.js";
import type { ReceiptLog } from "../src/evm/wallet.js";
import { Client, InMemorySwapStorage } from "../src/index.js";
import type { StoredSwap } from "../src/storage/types.js";
import {
  COORDINATOR,
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
function nativeLog(sender = COORDINATOR): ReceiptLog {
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
      fundTxid: overrides.fundTxid,
    }) as object),
    evm_chain_id: 30,
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
        response: nativeSwapResponse(),
      }),
    );
    const client = await Client.builder().withSwapStorage(storage).build();
    return { client, storage };
  }

  it("sends one payable executeAndCreate and never asks for Permit2 params", async () => {
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
    expect(sent[0].to).toBe(COORDINATOR);
    expect(sent[0].value).toBe(AMOUNT_WEI);
    // Rootstock rejects typed transactions
    expect(sent[0].type).toBe("legacy");
    expect(sent[0].data.startsWith(NATIVE_EXECUTE_AND_CREATE_SELECTOR)).toBe(
      true,
    );
    // simulated with the same value, so a short wallet fails before signing
    expect(simulated).toHaveLength(1);
    expect(simulated[0].value).toBe(AMOUNT_WEI);
    expect(await storage.get(SWAP_ID)).toMatchObject({
      evmFundTxid: SENT,
      evmCoordinatorAddress: COORDINATOR,
    });
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
      signer: signerWith({ chainId: 30, logs: [nativeLog()] }),
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
