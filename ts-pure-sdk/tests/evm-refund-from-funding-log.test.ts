import { decodeEventLog, encodeFunctionData, parseAbiItem } from "viem";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  encodeDepositsCallData,
  encodeRefundTo,
} from "../src/evm/coordinator.js";
import {
  decodeSwapCreatedLog,
  encodeHtlcErc20IsActiveCallData,
  findSwapCreated,
} from "../src/evm/htlc.js";
import type { GetSwapResponse } from "../src/index.js";
import { Client, InMemorySwapStorage } from "../src/index.js";
import type { StoredSwap } from "../src/storage/types.js";
import {
  COORDINATOR,
  FUND_TXID,
  HASH_LOCK,
  HTLC,
  HTLC_REFUND_SELECTOR,
  logFrom,
  REAL_LOG,
  REFUND_TO_SELECTOR,
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

const swapCreatedAbi = parseAbiItem(
  "event SwapCreated(bytes32 indexed preimageHash, address indexed refundAddress, address indexed claimAddress, address token, uint256 amount, uint256 timelock, bytes32 key)",
);
const refundToAbi = parseAbiItem(
  "function refundTo(bytes32 preimageHash, uint256 amount, address token, address claimAddress, uint256 timelock)",
);
const isActiveAbi = parseAbiItem(
  "function isActive(bytes32 preimageHash, uint256 amount, address token, address sender, address claimAddress, uint256 timelock) view returns (bool)",
);
const depositsAbi = parseAbiItem(
  "function deposits(bytes32 key) view returns (address)",
);

function decodedFixture() {
  const created = decodeSwapCreatedLog(REAL_LOG);
  if (!created) throw new Error("fixture did not decode");
  return created;
}

describe("decodeSwapCreatedLog", () => {
  it("matches viem's decoder on a real log", () => {
    const ours = decodeSwapCreatedLog(REAL_LOG);
    const ref = decodeEventLog({
      abi: [swapCreatedAbi],
      data: REAL_LOG.data as `0x${string}`,
      topics: REAL_LOG.topics as [`0x${string}`, ...`0x${string}`[]],
    }).args;

    expect(ours).toBeDefined();
    expect(ours?.preimageHash).toBe(ref.preimageHash);
    expect(ours?.refundAddress).toBe(ref.refundAddress.toLowerCase());
    expect(ours?.claimAddress).toBe(ref.claimAddress.toLowerCase());
    expect(ours?.token).toBe(ref.token.toLowerCase());
    expect(ours?.amount).toBe(ref.amount);
    expect(ours?.timelock).toBe(Number(ref.timelock));
    expect(ours?.key).toBe(ref.key);
    expect(ours?.amount).toBe(20305n);
  });

  it("ignores logs of other events", () => {
    const transfer = {
      topics: [
        "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
        REAL_LOG.topics[2],
        REAL_LOG.topics[3],
      ],
      data: `0x${"0".repeat(64)}`,
    };
    expect(decodeSwapCreatedLog(transfer)).toBeUndefined();
  });
});

describe("findSwapCreated", () => {
  const filter = {
    htlcAddress: HTLC,
    hashLock: HASH_LOCK,
    claimAddress: SERVER,
    senders: [COORDINATOR],
    token: WBTC,
  };

  it("keeps the log that matches the swap on every field", () => {
    expect(findSwapCreated([REAL_LOG], filter)).toHaveLength(1);
    expect(findSwapCreated([REAL_LOG], filter)[0].amount).toBe(20305n);
  });

  it("drops a log from another contract, sender, claimer or token", () => {
    expect(findSwapCreated([{ ...REAL_LOG, address: USDC }], filter)).toEqual(
      [],
    );
    expect(findSwapCreated([logFrom(SIGNER_ADDRESS)], filter)).toEqual([]);
    expect(
      findSwapCreated([logFrom(COORDINATOR, SIGNER_ADDRESS)], filter),
    ).toEqual([]);
    expect(findSwapCreated([REAL_LOG], { ...filter, token: USDC })).toEqual([]);
  });

  it("accepts any sender when none is given", () => {
    const anySender = { ...filter, senders: undefined };
    expect(findSwapCreated([logFrom(SIGNER_ADDRESS)], anySender)).toHaveLength(
      1,
    );
  });
});

describe("calldata encoders", () => {
  it("refundTo matches viem's ABI encoding for the decoded log", () => {
    const created = decodedFixture();
    const ours = encodeRefundTo(created.refundAddress, created);
    const ref = encodeFunctionData({
      abi: [refundToAbi],
      functionName: "refundTo",
      args: [
        created.preimageHash as `0x${string}`,
        created.amount,
        created.token as `0x${string}`,
        created.claimAddress as `0x${string}`,
        BigInt(created.timelock),
      ],
    });
    expect(ours.data).toBe(ref);
    expect(ours.to).toBe(COORDINATOR);
  });

  it("isActive matches viem's ABI encoding for the decoded log", () => {
    const created = decodedFixture();
    const ours = encodeHtlcErc20IsActiveCallData(HTLC, created);
    const ref = encodeFunctionData({
      abi: [isActiveAbi],
      functionName: "isActive",
      args: [
        created.preimageHash as `0x${string}`,
        created.amount,
        created.token as `0x${string}`,
        created.refundAddress as `0x${string}`,
        created.claimAddress as `0x${string}`,
        BigInt(created.timelock),
      ],
    });
    expect(ours.data).toBe(ref);
    expect(ours.to).toBe(HTLC);
  });

  it("deposits matches viem's ABI encoding for the decoded log's key", () => {
    const created = decodedFixture();
    const ours = encodeDepositsCallData(COORDINATOR, created.key);
    const ref = encodeFunctionData({
      abi: [depositsAbi],
      functionName: "deposits",
      args: [created.key as `0x${string}`],
    });
    expect(ours.data).toBe(ref);
    expect(ours.to).toBe(COORDINATOR);
  });
});

describe("refundSwap for an EVM-sourced swap", () => {
  const calldataFromServer = {
    coordinator_address: COORDINATOR,
    calldata: "0x29240837aa",
  };

  async function clientWith(
    stored: StoredSwap,
    storage = new InMemorySwapStorage(),
  ) {
    await storage.store(stored);
    const client = await Client.builder().withSwapStorage(storage).build();
    return { client, storage };
  }

  afterEach(() => vi.unstubAllGlobals());

  it("builds refundTo from the funding receipt and never asks the server", async () => {
    const fetchMock = serverGone();
    const { client } = await clientWith(storedSwap());

    const result = await client.refundSwap(SWAP_ID, { signer: signerWith() });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.to).toBe(COORDINATOR);
    expect(result.evmRefundData?.data.startsWith(REFUND_TO_SELECTOR)).toBe(
      true,
    );
    // the on-chain amount, not source_amount or target_amount
    expect(result.evmRefundData?.data).toContain(`${"0".repeat(60)}4f51`);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("uses the evm_to_arkade record's hash lock field", async () => {
    serverGone();
    const { client } = await clientWith(
      storedSwap({ direction: "evm_to_arkade" }),
    );

    const result = await client.refundSwap(SWAP_ID, { signer: signerWith() });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.to).toBe(COORDINATOR);
  });

  it("takes the expiry from the log the calldata commits to", async () => {
    // A directly created HTLC can carry another timelock than the record;
    // the refund reports the one it encodes.
    serverGone();
    const { client } = await clientWith(
      storedSwap({ refundLocktime: 4102444800 }),
    );

    const result = await client.refundSwap(SWAP_ID, { signer: signerWith() });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.timelockExpiry).toBe(TIMELOCK);
    expect(result.evmRefundData?.timelockExpired).toBe(true);
  });

  it("refunds an HTLC the signer created itself straight from the HTLC contract", async () => {
    serverGone();
    const { client } = await clientWith(storedSwap());
    const signer = signerWith({ logs: [logFrom(SIGNER_ADDRESS)] });

    const result = await client.refundSwap(SWAP_ID, { signer });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.to).toBe(HTLC);
    expect(result.evmRefundData?.data.startsWith(HTLC_REFUND_SELECTOR)).toBe(
      true,
    );
    expect(result.evmRefundData?.data).toContain(`${"0".repeat(60)}4f51`);
  });

  it("reports an HTLC that is no longer active instead of encoding a refund", async () => {
    const fetchMock = serverGone();
    const { client } = await clientWith(storedSwap());

    const result = await client.refundSwap(SWAP_ID, {
      signer: signerWith({ active: false }),
    });

    expect(result.success).toBe(false);
    expect(result.message).toMatch(/no longer active/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("falls back to the server when the active check answers no data", async () => {
    // A node without the contract's code answers an empty call; that is not
    // a verdict on the HTLC.
    const fetchMock = serverAnswering({ calldata: calldataFromServer });
    const { client } = await clientWith(storedSwap());

    const result = await client.refundSwap(SWAP_ID, {
      signer: signerWith({ call: async () => "0x" }),
    });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.data).toBe("0x29240837aa");
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("refuses a signer on another chain before touching it", async () => {
    serverGone();
    const { client } = await clientWith(storedSwap());
    const signer = signerWith({
      chainId: 1,
      getTransaction: () => Promise.reject(new Error("must not be called")),
    });

    await expect(client.refundSwap(SWAP_ID, { signer })).rejects.toThrow(
      /on chain 1 but swap .* on chain 137/,
    );
  });

  it("fails clearly when the receipt has no matching SwapCreated log", async () => {
    serverGone();
    const { client } = await clientWith(storedSwap());

    const result = await client.refundSwap(SWAP_ID, {
      signer: signerWith({ logs: [] }),
    });

    expect(result.success).toBe(false);
    expect(result.message).toMatch(/no SwapCreated log/);
  });

  it("names the signer contract when the receipt carries no logs", async () => {
    // An EvmSigner written before this path existed returns a bare receipt.
    // That must read as a contract gap, not as an iteration error.
    serverGone();
    const { client } = await clientWith(storedSwap());
    const legacy = signerWith({
      waitForReceipt: async (hash) => ({
        status: "success",
        blockNumber: 1n,
        transactionHash: hash,
      }),
    });

    const result = await client.refundSwap(SWAP_ID, { signer: legacy });

    expect(result.success).toBe(false);
    expect(result.message).toMatch(/waitForReceipt/);
  });

  it("does not ask the server for another tx when the receipt was readable", async () => {
    // A receipt without logs is a signer property; another txid cannot
    // change it, so the only request is the calldata fallback.
    const fetchMock = serverAnswering({ calldata: calldataFromServer });
    const { client } = await clientWith(storedSwap());
    const legacy = signerWith({
      waitForReceipt: async (hash) => ({
        status: "success",
        blockNumber: 1n,
        transactionHash: hash,
      }),
    });

    const result = await client.refundSwap(SWAP_ID, { signer: legacy });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.data).toBe("0x29240837aa");
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("names a reverted funding tx as such", async () => {
    serverGone();
    const { client } = await clientWith(storedSwap());
    const signer = signerWith({
      waitForReceipt: async (hash) => ({
        status: "reverted",
        blockNumber: 1n,
        transactionHash: hash,
        logs: [],
      }),
    });

    const result = await client.refundSwap(SWAP_ID, { signer });

    expect(result.success).toBe(false);
    expect(result.message).toMatch(/reverted/);
  });

  it("asks the server for the funding tx when it has no record of it", async () => {
    // The stored copy dates from creation, so it names no funding tx. One
    // refresh recovers it; the calldata endpoint is still never hit.
    const fetchMock = serverAnswering({
      swap: swapResponse({ fundTxid: FUND_TXID }),
    });
    const { client, storage } = await clientWith(
      storedSwap({ evmFundTxid: undefined }),
    );

    const result = await client.refundSwap(SWAP_ID, { signer: signerWith() });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.to).toBe(COORDINATOR);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(await storage.get(SWAP_ID)).toMatchObject({
      evmFundTxid: FUND_TXID,
      response: { evm_fund_txid: FUND_TXID },
    });
  });

  it("uses the stored copy's funding tx before asking the server", async () => {
    // A wizard poll stored the server's copy after the monitor indexed the
    // funding; that is enough to refund with the server gone.
    const fetchMock = serverGone();
    const { client, storage } = await clientWith(
      storedSwap({ evmFundTxid: undefined, responseFundTxid: FUND_TXID }),
    );

    const result = await client.refundSwap(SWAP_ID, { signer: signerWith() });

    expect(result.success).toBe(true);
    expect(fetchMock).not.toHaveBeenCalled();
    expect((await storage.get(SWAP_ID))?.evmFundTxid).toBe(FUND_TXID);
  });

  it("names the gap when no funding tx is known and the server is gone", async () => {
    serverGone();
    const { client } = await clientWith(storedSwap({ evmFundTxid: undefined }));

    const result = await client.refundSwap(SWAP_ID, { signer: signerWith() });

    expect(result.success).toBe(false);
    expect(result.message).toMatch(/no funding tx is known/);
  });

  it("keeps the server's funding tx even when storing the refreshed copy fails", async () => {
    serverAnswering({ swap: swapResponse({ fundTxid: FUND_TXID }) });
    class ReadOnlyStorage extends InMemorySwapStorage {
      override async update(): Promise<void> {
        throw new Error("quota exceeded");
      }
    }
    const { client } = await clientWith(
      storedSwap({ evmFundTxid: undefined }),
      new ReadOnlyStorage(),
    );

    const result = await client.refundSwap(SWAP_ID, { signer: signerWith() });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.to).toBe(COORDINATOR);
  });

  it("skips a recorded tx the node does not know and keeps the one that worked", async () => {
    // The wallet repriced the funding after this client recorded it. The
    // node no longer knows that hash; the server's monitor saw the mined one.
    const stale = `0x${"ab".repeat(32)}`;
    const fetchMock = serverAnswering({
      swap: swapResponse({ fundTxid: FUND_TXID }),
    });
    const { client, storage } = await clientWith(
      storedSwap({ evmFundTxid: stale }),
    );

    const result = await client.refundSwap(SWAP_ID, {
      signer: signerWith({ unknownHashes: [stale] }),
    });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.to).toBe(COORDINATOR);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect((await storage.get(SWAP_ID))?.evmFundTxid).toBe(FUND_TXID);
  });

  it("falls back to the server's calldata when the chain read fails", async () => {
    // A transient RPC failure must not fail a refund the server can still
    // build; the chain path is preferred, not mandatory.
    const fetchMock = serverAnswering({
      swap: swapResponse({ fundTxid: FUND_TXID }),
      calldata: calldataFromServer,
    });
    const { client } = await clientWith(storedSwap());
    const flaky = signerWith({
      waitForReceipt: () => Promise.reject(new Error("rpc down")),
    });

    const result = await client.refundSwap(SWAP_ID, { signer: flaky });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.data).toBe("0x29240837aa");
    // one refresh for the server's txid, then the calldata
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("returns both reasons when the chain read and the server both fail", async () => {
    serverGone();
    const { client } = await clientWith(storedSwap());
    const flaky = signerWith({
      waitForReceipt: () => Promise.reject(new Error("rpc down")),
    });

    const result = await client.refundSwap(SWAP_ID, { signer: flaky });

    expect(result.success).toBe(false);
    expect(result.message).toMatch(/rpc down.*server is gone/s);
  });

  it("ignores a decoy SwapCreated from another sender or for another claimer", async () => {
    // A DEX call inside the funding tx could create HTLCs with our hash lock
    // before the coordinator creates the real one, so they land first in
    // the receipt. Neither the signer nor the coordinator created them.
    serverGone();
    const attacker = "0x1111111111111111111111111111111111111111";
    const { client } = await clientWith(storedSwap());
    const signer = signerWith({
      logs: [logFrom(attacker, attacker), logFrom(attacker), REAL_LOG],
    });

    const result = await client.refundSwap(SWAP_ID, { signer });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.to).toBe(COORDINATOR);
  });

  it("refuses to guess between the signer's own HTLC and the coordinator's", async () => {
    serverGone();
    const { client } = await clientWith(storedSwap());
    const signer = signerWith({ logs: [logFrom(SIGNER_ADDRESS), REAL_LOG] });

    const result = await client.refundSwap(SWAP_ID, { signer });

    expect(result.success).toBe(false);
    expect(result.message).toMatch(/refusing to guess.*server is gone/s);
  });

  it("asks the sender for its deposit when the coordinator is not on record", async () => {
    // A swap not funded through this SDK carries no coordinator address, so
    // the log's sender is trusted only if it holds the deposit for the HTLC.
    serverGone();
    const { client } = await clientWith(
      storedSwap({ evmCoordinatorAddress: undefined }),
    );

    const trusted = await client.refundSwap(SWAP_ID, { signer: signerWith() });
    expect(trusted.success).toBe(true);
    expect(trusted.evmRefundData?.to).toBe(COORDINATOR);

    // A fresh record, since the call above has just put the coordinator on
    // this one; an EOA answers deposits() with no data.
    const { client: another } = await clientWith(
      storedSwap({ evmCoordinatorAddress: undefined }),
    );
    const untrusted = await another.refundSwap(SWAP_ID, {
      signer: signerWith({ deposit: false }),
    });
    expect(untrusted.success).toBe(false);
    expect(untrusted.message).toMatch(/deposits returned no data/);
  });

  it("remembers the coordinator the deposits probe proved", async () => {
    serverGone();
    const { client, storage } = await clientWith(
      storedSwap({ evmCoordinatorAddress: undefined }),
    );
    const first = await client.refundSwap(SWAP_ID, { signer: signerWith() });
    expect(first.success).toBe(true);
    expect(await storage.get(SWAP_ID)).toMatchObject({
      evmCoordinatorAddress: COORDINATOR,
    });
    // From now on only that coordinator's log counts, even when another
    // sender holds a deposit for the same hash lock.
    const stranger = `0x${"5".repeat(40)}`;
    const second = await client.refundSwap(SWAP_ID, {
      signer: signerWith({ logs: [logFrom(stranger)] }),
    });
    expect(second.success).toBe(false);
    expect(second.message).toMatch(new RegExp(`created by .*${COORDINATOR}`));
  });

  it("trusts the coordinator the server names on the Lightning direction", async () => {
    // EVM -> Lightning responses carry evm_coordinator_address, so a swap this
    // SDK did not fund needs no deposits probe there; an EOA-like answer to
    // deposits() would otherwise reject the sender.
    serverGone();
    const { client } = await clientWith(
      storedSwap({
        direction: "evm_to_lightning",
        evmCoordinatorAddress: undefined,
      }),
    );

    const result = await client.refundSwap(SWAP_ID, {
      signer: signerWith({ deposit: false }),
    });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.to).toBe(COORDINATOR);
  });

  it("still asks the server when no signer is given", async () => {
    const fetchMock = serverAnswering({ calldata: calldataFromServer });
    const { client } = await clientWith(storedSwap());

    const result = await client.refundSwap(SWAP_ID);

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.data).toBe("0x29240837aa");
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("reports a server failure as a result when no signer is given", async () => {
    serverGone();
    const { client } = await clientWith(storedSwap());

    const result = await client.refundSwap(SWAP_ID);

    expect(result.success).toBe(false);
    expect(result.message).toMatch(
      /Failed to fetch refund calldata: .*server is gone/,
    );
  });

  it("does not let a server refresh forget the funding it recorded", async () => {
    // The server's copy names no funding tx until its monitor has indexed
    // it; a refresh in that window must not wipe what fundSwap recorded.
    serverAnswering({ swap: swapResponse({ fundTxid: null }) });
    const { client, storage } = await clientWith(storedSwap());

    const fresh: GetSwapResponse = await client.getSwap(SWAP_ID, {
      updateStorage: true,
    });

    expect(fresh).toMatchObject({ evm_fund_txid: null });
    expect(await storage.get(SWAP_ID)).toMatchObject({
      evmFundTxid: FUND_TXID,
      evmCoordinatorAddress: COORDINATOR,
      response: { evm_fund_txid: null },
    });
  });
});
