import { encodeFunctionData, parseAbi } from "viem";
import { afterEach, describe, expect, it, vi } from "vitest";
import { buildCctpInboundBatch } from "../src/cctp-inbound/userOp.js";
import { computeCoordinatorCallsHash } from "../src/evm/coordinator.js";
import {
  checkFundingPayload,
  type FundingPayload,
  type FundingPins,
} from "../src/evm/funding-pins.js";
import { Client, InMemorySwapStorage } from "../src/index.js";
import {
  COORDINATOR,
  fundingTerms,
  HASH_LOCK,
  SERVER,
  SIGNER_ADDRESS,
  SWAP_ID,
  serverAnswering,
  signerWith,
  storedSwap,
  swapResponse,
  TIMELOCK,
  USDC,
  WBTC,
} from "./evm-funding-fixture.js";

const ATTACKER = "0xa77ac4e7a77ac4e7a77ac4e7a77ac4e7a77ac4e7";
const ROUTER = "0x1111111254eeb25477b68fb85ed929f73a960582";
const INPUT = 15658166n;
const MNEMONIC =
  "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

const erc20 = (fn: "transfer" | "approve", to: string, amount: bigint) =>
  encodeFunctionData({
    abi: parseAbi([`function ${fn}(address,uint256)`]),
    args: [to as `0x${string}`, amount],
  });
const call = (target: string, callData: string) => ({
  target,
  value: "0",
  call_data: callData,
});

/** An honest payload for the fixture swap, with `overrides` and a matching calls hash. */
function payload(overrides: Partial<FundingPayload> = {}): FundingPayload {
  const calls = overrides.calls ?? [
    call(USDC, erc20("approve", ROUTER, INPUT)),
    call(ROUTER, "0x12345678"),
  ];
  return {
    coordinator_address: COORDINATOR,
    permit2_address: "0x000000000022d473030f116ddee9f6b43ac78ba3",
    source_token_address: USDC,
    source_amount: INPUT.toString(),
    lock_token_address: WBTC,
    preimage_hash: HASH_LOCK,
    claim_address: SERVER,
    timelock: TIMELOCK,
    ...overrides,
    calls,
    calls_hash: computeCoordinatorCallsHash(
      calls.map((c) => ({
        target: c.target,
        value: BigInt(c.value),
        data: c.call_data,
      })),
    ),
  };
}

/** Each hostile payload, the pins in force, and why it is refused. */
const HOSTILE: [string, FundingPayload, FundingPins, string][] = [
  [
    "an attacker's contract as the Permit2 spender",
    payload({ coordinator_address: ATTACKER }),
    {},
    "not the swap's coordinator",
  ],
  [
    "an unknown lock token",
    payload({ lock_token_address: ATTACKER }),
    {},
    "not the swap's lock token",
  ],
  [
    "an amount above the input",
    payload({ source_amount: (INPUT + 1n).toString() }),
    {},
    "more than the input",
  ],
  [
    "the input transferred to an attacker",
    payload({
      calls: [
        call(USDC, erc20("approve", ROUTER, INPUT)),
        call(USDC, erc20("transfer", ATTACKER, INPUT)),
        call(ROUTER, "0x12345678"),
      ],
    }),
    {},
    "other than a relay fee or an approval",
  ],
  [
    "the input transferred to an attacker as the first call",
    payload({ calls: [call(USDC, erc20("transfer", ATTACKER, INPUT))] }),
    {},
    "only a relay fee",
  ],
  [
    "the input approved to an attacker under a pin",
    payload({
      calls: [
        call(USDC, erc20("approve", ATTACKER, INPUT)),
        call(ROUTER, "0x12345678"),
      ],
    }),
    { allowedCallTargets: [ROUTER] },
    "which is not pinned",
  ],
  [
    "the input approved to an address the batch never calls",
    payload({
      calls: [
        call(USDC, erc20("approve", ATTACKER, INPUT)),
        call(ROUTER, "0x12345678"),
      ],
    }),
    {},
    "which the batch never calls",
  ],
  [
    "a call target outside the pins",
    payload({
      calls: [
        call(USDC, erc20("approve", ROUTER, INPUT)),
        call(ATTACKER, "0x12345678"),
      ],
    }),
    { allowedCallTargets: [ROUTER] },
    "which is not pinned",
  ],
  [
    "a relay fee above the cap",
    payload({
      relay_fee: "1000",
      calls: [
        call(USDC, erc20("transfer", SERVER, 1000n)),
        call(USDC, erc20("approve", ROUTER, INPUT)),
        call(ROUTER, "0x12345678"),
      ],
    }),
    { maxRelayFee: 999n },
    "only a relay fee of at most 999",
  ],
  [
    "a call moving the lock token",
    payload({
      calls: [
        call(USDC, erc20("approve", ROUTER, INPUT)),
        call(ROUTER, "0x12345678"),
        call(WBTC, erc20("transfer", ATTACKER, 1n)),
      ],
    }),
    {},
    "calls the lock token",
  ],
];

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("checkFundingPayload", () => {
  it("passes an honest payload and signs the swap's terms", () => {
    const checked = checkFundingPayload(fundingTerms(), HASH_LOCK, payload());
    expect(checked.coordinator).toBe(COORDINATOR);
    expect(checked.lockToken).toBe(WBTC);
    expect(checked.sourceAmount).toBe(INPUT);
  });

  it("passes a relay fee to the server within the cap", () => {
    const fee = payload({
      relay_fee: "1000",
      calls: [
        call(USDC, erc20("transfer", SERVER, 1000n)),
        call(USDC, erc20("approve", ROUTER, INPUT)),
        call(ROUTER, "0x12345678"),
      ],
    });
    expect(() =>
      checkFundingPayload(fundingTerms(), HASH_LOCK, fee, {
        maxRelayFee: 1000n,
      }),
    ).not.toThrow();
  });

  it("refuses a relay fee by default", () => {
    const fee = payload({
      relay_fee: "1",
      calls: [call(USDC, erc20("transfer", SERVER, 1n))],
    });
    expect(() => checkFundingPayload(fundingTerms(), HASH_LOCK, fee)).toThrow(
      "at most 0",
    );
  });

  it.each(HOSTILE)("refuses %s", (_, hostile, pins, reason) => {
    expect(() =>
      checkFundingPayload(fundingTerms(), HASH_LOCK, hostile, pins),
    ).toThrow(reason);
  });
});

describe("every funding path refuses a payload not bound to the swap", () => {
  const requested = (fetchMock: ReturnType<typeof vi.fn>, path: string) =>
    fetchMock.mock.calls.some(([input]) =>
      (input instanceof Request ? input.url : String(input)).includes(path),
    );
  async function client() {
    const storage = new InMemorySwapStorage();
    await storage.store(storedSwap({ evmFundTxid: undefined }));
    return Client.builder()
      .withSwapStorage(storage)
      .withMnemonic(MNEMONIC)
      .build();
  }

  it.each(
    HOSTILE,
  )("fundSwapGasless: %s, before the relay is called", async (_, hostile, pins, reason) => {
    const fetchMock = serverAnswering({
      swap: swapResponse(),
      permit2: hostile,
      gasless: { id: SWAP_ID, status: "clientfunded", tx_hash: "0x01" },
    });
    await expect(
      (await client()).fundSwapGasless(SWAP_ID, { funding: pins }),
    ).rejects.toThrow(reason);
    expect(requested(fetchMock, "fund-gasless")).toBe(false);
  });

  it.each(
    HOSTILE,
  )("getCoordinatorFundingCallDataPermit2: %s", async (_, hostile, pins, reason) => {
    serverAnswering({ swap: swapResponse(), permit2: hostile });
    await expect(
      (await client()).getCoordinatorFundingCallDataPermit2(SWAP_ID, 137, {
        funding: pins,
      }),
    ).rejects.toThrow(reason);
  });

  it.each(
    HOSTILE,
  )("fundSwap (wallet Permit2): %s, before the wallet signs", async (_, hostile, pins, reason) => {
    serverAnswering({ swap: swapResponse(), permit2: hostile });
    const signTypedData = vi.fn(async () => `0x${"11".repeat(64)}1b`);
    const sendTransaction = vi.fn(async () => `0x${"aa".repeat(32)}`);
    await expect(
      (await client()).fundSwap(
        SWAP_ID,
        signerWith({ signTypedData, sendTransaction }),
        { funding: pins },
      ),
    ).rejects.toThrow(reason);
    expect(signTypedData).not.toHaveBeenCalled();
  });

  it.each(
    HOSTILE,
  )("CCTP / Solana / continuation batch: %s, before signing", async (_, hostile, pins, reason) => {
    const signTypedData = vi.fn(async () => `0x${"11".repeat(65)}`);
    await expect(
      buildCctpInboundBatch({
        server: {
          ...hostile,
          permit2_address: hostile.permit2_address ?? "",
          aa: { entry_point: "0x", delegation_target: "0x" },
        },
        expected: {
          hashLock: HASH_LOCK,
          terms: fundingTerms({ chainId: 42161 }),
          pins,
        },
        smartAccountAddress: SIGNER_ADDRESS as `0x${string}`,
        signTypedData: signTypedData as never,
        cctpMessage: "0x",
        cctpAttestation: "0x",
        chainId: 42161,
      }),
    ).rejects.toThrow(reason);
    expect(signTypedData).not.toHaveBeenCalled();
  });

  it("signs the swap's terms, not the payload's, on an honest payload", async () => {
    serverAnswering({ swap: swapResponse(), permit2: payload() });
    const params = await (await client()).getPermit2FundingParamsUnsigned(
      SWAP_ID,
      137,
      SIGNER_ADDRESS,
    );
    expect(params.coordinatorAddress).toBe(COORDINATOR);
    expect(params.typedData.domain.verifyingContract.toLowerCase()).toBe(
      "0x000000000022d473030f116ddee9f6b43ac78ba3",
    );
    expect(params.lockTokenAddress).toBe(WBTC);
  });

  it("refuses a swap recovered without funding terms", async () => {
    serverAnswering({ swap: swapResponse(), permit2: payload() });
    const storage = new InMemorySwapStorage();
    await storage.store(storedSwap({ evmFundingTerms: undefined }));
    const recovered = await Client.builder()
      .withSwapStorage(storage)
      .withMnemonic(MNEMONIC)
      .build();
    await expect(
      recovered.getPermit2FundingParamsUnsigned(SWAP_ID, 137, SIGNER_ADDRESS),
    ).rejects.toThrow("no funding terms recorded");
  });
});
