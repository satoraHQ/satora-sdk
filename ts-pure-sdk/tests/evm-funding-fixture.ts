import { vi } from "vitest";
import { SWAP_CREATED_TOPIC } from "../src/evm/htlc.js";
import type { EvmSigner, ReceiptLog } from "../src/evm/wallet.js";
import { addressToBytes32 } from "../src/index.js";
import { SWAP_STORAGE_VERSION } from "../src/storage/index.js";
import type { StoredSwap } from "../src/storage/types.js";

/**
 * A real `SwapCreated` log from a Permit2-funded USDC -> BTC swap on the
 * Polygon regtest fork. The locked amount (20305) is what the DEX swap
 * produced: it is neither the swap's source_amount (15658166) nor its
 * target_amount (20001), so nothing in the swap record can stand in for it.
 */
export const HTLC = "0xba5ad2d7eea72679cc3b11ca52fc3d212b16d035";
export const COORDINATOR = "0x3a4cbee89a62c45d57c774527f931f0b67212c28";
export const SERVER = "0xfcb0b3c3c6320420b749bcd3e4400f0b9fdf682a";
export const SIGNER_ADDRESS = "0x90f79bf6eb2c4f870365e785982e1f101e93b906";
export const USDC = "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359";
export const WBTC = "0x1bfd67037b42cf73acf2047067bd4f2c47d9bfd6";
export const HASH_LOCK =
  "0xea5d9d3877df7b3ba946babd6008e18266b90aaa7db3f0ac0e2aa1677dd5fb92";
export const FUND_TXID =
  "0xae8c65b01783ba28f572e18417a86826f713b951e1634f06a258e65b7ebd35e1";
export const TIMELOCK = 1788445287;
export const SWAP_ID = "2c30cf56-bb61-4ccb-8e3d-d38971a54922";
export const REAL_LOG: ReceiptLog = {
  address: HTLC,
  topics: [
    SWAP_CREATED_TOPIC,
    HASH_LOCK,
    addressToBytes32(COORDINATOR),
    addressToBytes32(SERVER),
  ],
  data: "0x0000000000000000000000001bfd67037b42cf73acf2047067bd4f2c47d9bfd60000000000000000000000000000000000000000000000000000000000004f51000000000000000000000000000000000000000000000000000000006a99826735bc433a63eb3b0a4672ecf72368473f9147720c972c46945f661c92d47531ee",
};

export const REFUND_TO_SELECTOR = "0x29240837";
export const HTLC_REFUND_SELECTOR = "0x36504721";
const IS_ACTIVE_SELECTOR = "0x160c723c";
const DEPOSITS_SELECTOR = "0x3d4dff7b";
const BALANCE_OF_SELECTOR = "0x70a08231";
const ALLOWANCE_SELECTOR = "0xdd62ed3e";

const ZERO_WORD = `0x${"0".repeat(64)}`;
const TRUE_WORD = `0x${"0".repeat(63)}1`;
const MAX_WORD = `0x${"f".repeat(64)}`;

/** The same `SwapCreated` log as {@link REAL_LOG}, created by `sender`. */
export function logFrom(sender: string, claimAddress = SERVER): ReceiptLog {
  return {
    ...REAL_LOG,
    topics: [
      REAL_LOG.topics[0],
      REAL_LOG.topics[1],
      addressToBytes32(sender),
      addressToBytes32(claimAddress),
    ],
  };
}

export function swapResponse(
  overrides: {
    direction?: "evm_to_bitcoin" | "evm_to_arkade" | "evm_to_lightning";
    fundTxid?: string | null;
    refundLocktime?: number;
    clientEvmAddress?: string | null;
    gasless?: boolean;
  } = {},
): StoredSwap["response"] {
  const {
    direction = "evm_to_bitcoin",
    fundTxid = null,
    refundLocktime = TIMELOCK,
    clientEvmAddress = null,
    gasless = false,
  } = overrides;
  return {
    direction,
    id: SWAP_ID,
    status: "clientfundedserverrefunded",
    evm_chain_id: 137,
    evm_htlc_address: HTLC,
    ...(direction === "evm_to_bitcoin"
      ? { evm_hash_lock: HASH_LOCK }
      : { hash_lock: HASH_LOCK }),
    // Only the Lightning direction's response names the coordinator.
    ...(direction === "evm_to_lightning"
      ? { evm_coordinator_address: COORDINATOR }
      : {}),
    evm_fund_txid: fundTxid,
    evm_refund_locktime: refundLocktime,
    server_evm_address: SERVER,
    client_evm_address: clientEvmAddress,
    gasless,
    source_amount: "15658166",
    target_amount: "20001",
    source_token: {
      token_id: USDC,
      symbol: "USDC",
      chain: "137",
      name: "USDC",
      decimals: 6,
    },
  } as unknown as StoredSwap["response"];
}

/**
 * A stored swap as this SDK leaves it after funding through the coordinator:
 * the funding tx and the coordinator recorded beside the server's copy.
 */
export function storedSwap(
  overrides: Partial<StoredSwap> & {
    direction?: "evm_to_bitcoin" | "evm_to_arkade" | "evm_to_lightning";
    responseFundTxid?: string | null;
    refundLocktime?: number;
  } = {},
): StoredSwap {
  const { direction, responseFundTxid, refundLocktime, ...rest } = overrides;
  return {
    version: SWAP_STORAGE_VERSION,
    swapId: SWAP_ID,
    keyIndex: 0,
    publicKey: `0x${"02".repeat(33)}`,
    preimage: "00".repeat(32),
    preimageHash: HASH_LOCK.slice(2),
    secretKey: "01".repeat(32),
    storedAt: 0,
    updatedAt: 0,
    evmFundTxid: FUND_TXID,
    evmCoordinatorAddress: COORDINATOR,
    response: swapResponse({
      direction,
      fundTxid: responseFundTxid,
      refundLocktime,
    }),
    ...rest,
  };
}

/**
 * A signer on the swap's chain whose node answers the way a real one does:
 * the HTLC is active, the coordinator's deposit belongs to the signer or to
 * the address `deposit` names (`false` answers like an EOA, with no data),
 * balances and allowances are unlimited, every receipt carries `logs`, and a
 * hash in `unknownHashes` is rejected by `getTransaction` while
 * `waitForReceipt` on it never settles, as viem and ethers behave.
 */
export function signerWith(
  overrides: Partial<EvmSigner> & {
    logs?: ReceiptLog[];
    active?: boolean;
    deposit?: boolean | string;
    unknownHashes?: string[];
  } = {},
): EvmSigner {
  const {
    logs = [REAL_LOG],
    active = true,
    deposit = true,
    unknownHashes = [],
    ...rest
  } = overrides;
  return {
    address: SIGNER_ADDRESS,
    chainId: 137,
    signTypedData: () => Promise.reject(new Error("not needed")),
    sendTransaction: () => Promise.reject(new Error("not needed")),
    getTransaction: async (hash) => {
      if (unknownHashes.includes(hash)) {
        throw new Error(`transaction ${hash} not found`);
      }
      return { to: HTLC, input: "0x", from: SIGNER_ADDRESS };
    },
    call: async ({ data }) => {
      if (data.startsWith(IS_ACTIVE_SELECTOR)) {
        return active ? TRUE_WORD : ZERO_WORD;
      }
      if (data.startsWith(DEPOSITS_SELECTOR)) {
        if (deposit === false) return "0x";
        return addressToBytes32(deposit === true ? SIGNER_ADDRESS : deposit);
      }
      if (
        data.startsWith(BALANCE_OF_SELECTOR) ||
        data.startsWith(ALLOWANCE_SELECTOR)
      ) {
        return MAX_WORD;
      }
      return "0x";
    },
    waitForReceipt: (hash) =>
      unknownHashes.includes(hash)
        ? new Promise(() => {})
        : Promise.resolve({
            status: "success",
            blockNumber: 1n,
            transactionHash: hash,
            logs,
          }),
    ...rest,
  };
}

/** Every request fails: the server is gone. */
export function serverGone(): ReturnType<typeof vi.fn> {
  const fetchMock = vi.fn(async () => {
    throw new Error("server is gone");
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

/**
 * A server answering `GET /swap/{id}` with `swap`, the Permit2 funding
 * params with `permit2`, the gasless relay with `gasless`, and the refund
 * calldata endpoint with `calldata`. Anything not given is a failure.
 */
export function serverAnswering(routes: {
  swap?: unknown;
  permit2?: unknown;
  gasless?: unknown;
  calldata?: unknown;
}): ReturnType<typeof vi.fn> {
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const url = input instanceof Request ? input.url : String(input);
    const answer = (body: unknown, what: string) => {
      if (body === undefined) throw new Error(`${what} must not be used`);
      return Response.json(body, { status: 200 });
    };
    if (url.includes("refund-and-swap-calldata")) {
      return answer(routes.calldata, "the refund calldata endpoint");
    }
    if (url.includes("swap-and-lock-calldata-permit2")) {
      return answer(routes.permit2, "the Permit2 params endpoint");
    }
    if (url.includes("fund-gasless")) {
      return answer(routes.gasless, "the gasless relay");
    }
    if (url.endsWith(`/swap/${SWAP_ID}`)) {
      return answer(routes.swap, "GET /swap/{id}");
    }
    throw new Error(`unexpected request ${url}`);
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}
