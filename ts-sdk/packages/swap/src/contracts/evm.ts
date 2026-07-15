/**
 * The `EvmContractManager`'s pure core.
 *
 * An EVM HTLC (an `HTLCErc20` swap) exposes its lifecycle as three events, all
 * indexed by the swap's `preimageHash`: `SwapCreated` (funded), `SwapRedeemed`
 * (claimed — reveals the preimage), and `SwapRefunded` (reclaimed after the
 * timelock). This maps a decoded event history to an {@link HtlcObservation}.
 */
import type { HtlcObservation } from "../actions/types.js";

/** One decoded `HTLCErc20` lifecycle event for a single swap's `preimageHash`. */
export type EvmHtlcEvent =
  | { kind: "created"; amount: bigint; token: `0x${string}` }
  | { kind: "redeemed"; preimage: `0x${string}` }
  | { kind: "refunded" };

/** An EVM HTLC's observation, plus the preimage if a claim revealed it. */
export type EvmHtlcState = {
  observation: HtlcObservation;
  preimage?: `0x${string}`;
};

/** What a `SwapCreated` must satisfy to be `confirmed` (else `invalid`). */
export type EvmExpectation = {
  /** Minimum locked amount (token's smallest unit). */
  amount: bigint;
  /** Expected token address; verified when provided. */
  token?: `0x${string}`;
};

/**
 * Reduce an HTLC's on-chain event history to an observation.
 *
 * A terminal spend wins over funding regardless of ordering: `SwapRedeemed`
 * reveals the preimage → `spent_claim` (plus the recovered preimage, which the
 * caller needs to settle the counterparty leg — as on Arkade); `SwapRefunded` →
 * `spent_refund`. A `SwapCreated` is `confirmed` only if it locks at least
 * `expected.amount` of the expected token — otherwise `invalid`, so the client
 * never claims a leg funded on the wrong terms. `pending` marks a broadcast-but-
 * unmined funding tx (`mempool`); otherwise `absent`.
 */
export function evmObservation(
  events: EvmHtlcEvent[],
  expected: EvmExpectation,
  pending = false,
): EvmHtlcState {
  const redeemed = events.find((e) => e.kind === "redeemed");
  if (redeemed?.kind === "redeemed")
    return { observation: "spent_claim", preimage: redeemed.preimage };
  if (events.some((e) => e.kind === "refunded"))
    return { observation: "spent_refund" };
  const created = events.find((e) => e.kind === "created");
  if (created?.kind === "created") {
    const amountOk = created.amount >= expected.amount;
    const tokenOk =
      expected.token === undefined ||
      created.token.toLowerCase() === expected.token.toLowerCase();
    return { observation: amountOk && tokenOk ? "confirmed" : "invalid" };
  }
  return { observation: pending ? "mempool" : "absent" };
}
