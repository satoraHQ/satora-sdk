/**
 * Binds a server funding payload (`swap-and-lock-calldata-permit2` /
 * `-userop`) to the swap this client created, before anything is signed.
 *
 * The payload names the Permit2 spender (the coordinator), the token it
 * locks and the calls the coordinator runs while it holds the deposit. A
 * server naming its own contract as spender, or a call handing the deposit
 * to itself, would drain the input without creating the swap's lock. So
 * the spender, HTLC, claimer, timelock and lock token come from the swap's
 * creation response, recorded here, the amount is bounded by its input,
 * and the calls are restricted. What gets signed is built from those terms,
 * not from the payload.
 */

import { computeCoordinatorCallsHash, PERMIT2_ADDRESS } from "./coordinator.js";
import { normalizeBytes32 } from "./htlc.js";
import { decodeUint256 } from "./wallet.js";

/**
 * What an EVM-sourced swap's funding must use, recorded from its creation
 * response and never updated from a later server answer.
 */
export interface EvmFundingTerms {
  chainId: number;
  /** The coordinator: the Permit2 spender. */
  coordinator: string;
  htlcAddress: string;
  htlcKind?: string;
  htlcVersion?: number;
  /** The server's address: the lock's claimer and the relay fee's payee. */
  claimAddress: string;
  timelock: number;
  /** The token the user pays with, on the chain the lock is on. */
  sourceToken: string;
  /** The input the user agreed to at creation, in the token's base units. */
  sourceAmount: string;
  /**
   * The token the coordinator locks. EVM → Arkade responses name none;
   * the payload's is then signed (the coordinator locks only the token the
   * witness names, so nothing else is at risk).
   */
  lockToken?: string;
}

/**
 * Optional pins on top of the recorded terms, never instead of them.
 */
export interface FundingPins {
  /**
   * Contracts the coordinator may call, and the only spenders an approval
   * of the input may name. Without it any target outside the source and
   * lock token is allowed (the SDK knows no router set) and an approval of
   * the input must name the target of a later call in the batch.
   */
  allowedCallTargets?: readonly string[];
  /**
   * Largest input the funding may take. Defaults to the input recorded at
   * creation; a payout-pinned swap re-quoted above it needs this raised.
   */
  maxSourceAmount?: bigint;
  /**
   * Largest relay fee (in source-token base units) the server may take from
   * the input as a transfer to itself. Defaults to 0: no fee.
   */
  maxRelayFee?: bigint;
}

/**
 * What a funding of one swap must match: the hash lock this client fixed
 * and the terms it recorded at creation, plus optional pins. Get it with
 * `Client.getEvmFundingExpectation`.
 */
export interface ExpectedFunding {
  hashLock: string;
  terms: EvmFundingTerms;
  pins?: FundingPins;
}

/** The parts of a funding payload the checks read. */
export interface FundingPayload {
  coordinator_address: string;
  permit2_address?: string;
  source_token_address: string;
  source_amount: string;
  lock_token_address: string;
  preimage_hash: string;
  claim_address: string;
  timelock: number;
  calls: Array<{ target: string; value: string; call_data: string }>;
  calls_hash: string;
  relay_fee?: string;
}

/** A payload checked against the swap, with what may be signed. */
export interface CheckedFunding {
  coordinator: string;
  htlcAddress: string;
  sourceToken: string;
  sourceAmount: bigint;
  lockToken: string;
  preimageHash: string;
  claimAddress: string;
  timelock: number;
  htlcKind?: string;
  htlcVersion?: number;
  calls: Array<{ target: string; value: bigint; data: string }>;
  /** Recomputed from `calls`. */
  callsHash: string;
}

// transfer(address,uint256), approve(address,uint256)
const TRANSFER_SELECTOR = "0xa9059cbb";
const APPROVE_SELECTOR = "0x095ea7b3";

const lower = (address: string) => address.toLowerCase();
const sameHash = (a: string, b: string) =>
  normalizeBytes32(a) === normalizeBytes32(b);

function refuse(reason: string): never {
  throw new Error(`Refusing the server's funding: ${reason}`);
}

/** Decodes `selector(address,uint256)` calldata, or undefined. */
function addressAmount(
  data: string,
  selector: string,
): { to: string; amount: bigint } | undefined {
  const clean = data.toLowerCase();
  if (!clean.startsWith(selector) || clean.length !== 10 + 128) {
    return undefined;
  }
  const word = clean.slice(10, 74);
  if (!/^0{24}/.test(word)) return undefined;
  return {
    to: `0x${word.slice(24)}`,
    amount: decodeUint256(clean.slice(74)),
  };
}

/**
 * Checks `payload` against `terms` (and the hash lock this client fixed)
 * and returns what may be signed. Throws before anything is signed for a
 * payload naming another spender, HTLC party, timelock, hash or lock token,
 * an amount above the input, or calls that could move the deposit.
 */
export function checkFundingPayload(
  terms: EvmFundingTerms,
  hashLock: string,
  payload: FundingPayload,
  pins: FundingPins = {},
): CheckedFunding {
  if (lower(payload.coordinator_address) !== lower(terms.coordinator)) {
    refuse(
      `it names spender ${payload.coordinator_address}, not the swap's coordinator ${terms.coordinator}`,
    );
  }
  if (
    payload.permit2_address !== undefined &&
    lower(payload.permit2_address) !== lower(PERMIT2_ADDRESS)
  ) {
    refuse(`it names Permit2 at ${payload.permit2_address}`);
  }
  if (!sameHash(payload.preimage_hash, hashLock)) {
    refuse(
      `it locks under hash ${payload.preimage_hash}, not the swap's ${hashLock}`,
    );
  }
  if (lower(payload.claim_address) !== lower(terms.claimAddress)) {
    refuse(
      `it names claimer ${payload.claim_address}, not the swap's ${terms.claimAddress}`,
    );
  }
  if (payload.timelock !== terms.timelock) {
    refuse(
      `it names timelock ${payload.timelock}, not the swap's ${terms.timelock}`,
    );
  }
  if (lower(payload.source_token_address) !== lower(terms.sourceToken)) {
    refuse(
      `it takes ${payload.source_token_address}, not the swap's source token ${terms.sourceToken}`,
    );
  }
  const lockToken = terms.lockToken ?? payload.lock_token_address;
  if (lower(payload.lock_token_address) !== lower(lockToken)) {
    refuse(
      `it locks ${payload.lock_token_address}, not the swap's lock token ${lockToken}`,
    );
  }
  const sourceAmount = BigInt(payload.source_amount);
  const limit = pins.maxSourceAmount ?? BigInt(terms.sourceAmount);
  if (sourceAmount <= 0n || sourceAmount > limit) {
    refuse(`it takes ${sourceAmount}, more than the input ${limit}`);
  }
  const calls = payload.calls.map((call) => ({
    target: call.target,
    value: BigInt(call.value),
    data: call.call_data,
  }));
  const callsHash = computeCoordinatorCallsHash(calls);
  if (!sameHash(callsHash, payload.calls_hash)) {
    refuse("its calls hash does not match its calls");
  }
  checkCalls(calls, {
    sourceToken: lower(terms.sourceToken),
    lockToken: lower(lockToken),
    feePayee: lower(terms.claimAddress),
    amount: sourceAmount,
    quotedFee:
      payload.relay_fee === undefined ? undefined : BigInt(payload.relay_fee),
    maxRelayFee: pins.maxRelayFee ?? 0n,
    allowed: pins.allowedCallTargets?.map(lower),
  });
  return {
    coordinator: terms.coordinator,
    htlcAddress: terms.htlcAddress,
    sourceToken: terms.sourceToken,
    sourceAmount,
    lockToken,
    preimageHash: `0x${normalizeBytes32(hashLock)}`,
    claimAddress: terms.claimAddress,
    timelock: terms.timelock,
    htlcKind: terms.htlcKind,
    htlcVersion: terms.htlcVersion,
    calls,
    callsHash,
  };
}

/**
 * The coordinator runs these calls holding the pulled input. No call may
 * carry native value. A call on the source token may only be the relay fee
 * (first, a transfer to the server within the cap and equal to the quoted
 * fee) or an approval of at most the input to a router the batch calls (or
 * a pinned one). No call may touch the lock token unless it is the source
 * token, which follows the same rule. Other targets must be pinned when
 * pins are given.
 */
function checkCalls(
  calls: Array<{ target: string; value: bigint; data: string }>,
  context: {
    sourceToken: string;
    lockToken: string;
    feePayee: string;
    amount: bigint;
    quotedFee?: bigint;
    maxRelayFee: bigint;
    allowed?: string[];
  },
): void {
  const targets = calls.map((call) => lower(call.target));
  calls.forEach((call, index) => {
    const target = targets[index];
    if (call.value !== 0n) refuse(`call ${index} sends native value`);
    if (target === context.lockToken && target !== context.sourceToken) {
      refuse(`call ${index} calls the lock token`);
    }
    if (target !== context.sourceToken) {
      if (context.allowed && !context.allowed.includes(target)) {
        refuse(`call ${index} targets ${call.target}, which is not pinned`);
      }
      return;
    }
    const fee =
      index === 0 ? addressAmount(call.data, TRANSFER_SELECTOR) : undefined;
    if (fee) {
      if (
        fee.to !== context.feePayee ||
        fee.amount > context.maxRelayFee ||
        (context.quotedFee !== undefined && fee.amount !== context.quotedFee)
      ) {
        refuse(
          `call 0 transfers ${fee.amount} of the input to ${fee.to}; only a relay fee of at most ${context.maxRelayFee} to the server is allowed`,
        );
      }
      return;
    }
    const approval = addressAmount(call.data, APPROVE_SELECTOR);
    if (!approval) {
      refuse(
        `call ${index} calls the source token for something other than a relay fee or an approval`,
      );
    }
    const spenderCalled = context.allowed
      ? context.allowed.includes(approval.to)
      : targets.slice(index + 1).includes(approval.to);
    if (approval.amount > context.amount || !spenderCalled) {
      refuse(
        `call ${index} approves ${approval.amount} of the input to ${approval.to}, ${
          approval.amount > context.amount
            ? "more than the input"
            : context.allowed
              ? "which is not pinned"
              : "which the batch never calls"
        }`,
      );
    }
  });
}
