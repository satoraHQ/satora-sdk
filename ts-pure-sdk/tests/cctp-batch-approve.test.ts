import { maxUint256 } from "viem";
import { describe, expect, it } from "vitest";
import {
  type BatchCall,
  buildCctpInboundBatch,
  PERMIT2_ADDRESS,
} from "../src/cctp-inbound/userOp.js";
import { computeCoordinatorCallsHash } from "../src/evm/coordinator.js";
import {
  COORDINATOR,
  fundingTerms,
  HASH_LOCK,
  SERVER,
  SIGNER_ADDRESS,
  TIMELOCK,
  USDC,
  WBTC,
} from "./evm-funding-fixture.js";

const INPUT = 15658166n;
const APPROVE_SELECTOR = "0x095ea7b3";

/** An honest UserOp calldata payload for the fixture swap on Arbitrum. */
function serverPayload() {
  return {
    coordinator_address: COORDINATOR,
    permit2_address: PERMIT2_ADDRESS.toLowerCase(),
    source_token_address: USDC,
    source_amount: INPUT.toString(),
    lock_token_address: WBTC,
    preimage_hash: HASH_LOCK,
    claim_address: SERVER,
    timelock: TIMELOCK,
    calls: [],
    calls_hash: computeCoordinatorCallsHash([]),
    aa: { entry_point: "0x", delegation_target: "0x" },
  };
}

async function builtCalls(permit2Allowance?: bigint): Promise<BatchCall[]> {
  const { calls } = await buildCctpInboundBatch({
    server: serverPayload(),
    expected: { hashLock: HASH_LOCK, terms: fundingTerms({ chainId: 42161 }) },
    smartAccountAddress: SIGNER_ADDRESS as `0x${string}`,
    signTypedData: async () => `0x${"11".repeat(64)}1b` as `0x${string}`,
    cctpMessage: "0x",
    cctpAttestation: "0x",
    chainId: 42161,
    skipReceiveMessage: true,
    permit2Allowance,
  });
  return calls;
}

function approveCalls(calls: BatchCall[]) {
  return calls.filter(
    (c) =>
      c.to.toLowerCase() === USDC.toLowerCase() &&
      c.data.startsWith(APPROVE_SELECTOR),
  );
}

function approvedAmount(call: BatchCall): bigint {
  return BigInt(`0x${call.data.slice(10 + 64)}`);
}

describe("the balance batch approves Permit2 as the account's allowance requires", () => {
  it("approves max when the allowance is unknown", async () => {
    const calls = await builtCalls(undefined);
    const approvals = approveCalls(calls);
    expect(approvals).toHaveLength(1);
    expect(approvedAmount(approvals[0])).toBe(maxUint256);
  });

  it("approves max on a fresh account with no allowance", async () => {
    const approvals = approveCalls(await builtCalls(0n));
    expect(approvals).toHaveLength(1);
    expect(approvedAmount(approvals[0])).toBe(maxUint256);
  });

  it("skips the approve when the allowance already covers the input", async () => {
    expect(approveCalls(await builtCalls(INPUT))).toHaveLength(0);
    expect(approveCalls(await builtCalls(maxUint256))).toHaveLength(0);
  });

  it("resets a smaller non-zero allowance to 0 before approving max", async () => {
    // USDT-family tokens revert a non-zero → non-zero approve; without the
    // reset the whole funding batch would revert on such a token.
    const approvals = approveCalls(await builtCalls(INPUT - 1n));
    expect(approvals).toHaveLength(2);
    expect(approvedAmount(approvals[0])).toBe(0n);
    expect(approvedAmount(approvals[1])).toBe(maxUint256);
  });

  it("always ends with the coordinator's executeAndCreateWithPermit2", async () => {
    for (const allowance of [undefined, 0n, INPUT - 1n, INPUT]) {
      const calls = await builtCalls(allowance);
      expect(calls.at(-1)?.to.toLowerCase()).toBe(COORDINATOR.toLowerCase());
    }
  });
});
