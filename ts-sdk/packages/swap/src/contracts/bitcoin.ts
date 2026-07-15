/**
 * The `BitcoinContractManager`'s pure core.
 *
 * A Bitcoin HTLC is an on-chain output at a witness-script address. It's observed
 * from esplora: an output paying the address means it's funded; a spend of that
 * output resolves it. The spend's witness reveals the preimage on a claim (a
 * timelock refund does not), so — script-agnostically, as on Arkade/EVM — a spend
 * is a claim iff a witness element hashes to the swap's `paymentHash`.
 */
import { hex } from "@scure/base";
import type { HtlcObservation } from "../actions/types.js";
import { preimageMatches } from "./preimage.js";

/** On-chain funding state of the HTLC address. */
export type BitcoinFunding = "absent" | "mempool" | "confirmed";

/** Facts gathered for an HTLC address from esplora. */
export type BitcoinHtlcFacts = {
  funding: BitcoinFunding;
  /** Total sats paid to the HTLC address by the funding tx (0 when absent). */
  fundedSats: number;
  /**
   * The witness stack (hex items) of the input that spent the HTLC output, if it
   * was spent. Absent while the output is still unspent.
   */
  spendWitness?: string[];
};

/** An HTLC's observation, plus the preimage if a claim revealed it. */
export type BitcoinHtlcState = {
  observation: HtlcObservation;
  preimage?: Uint8Array;
};

/**
 * Classify a spend from its witness: a claim path reveals the preimage in the
 * witness (verified against `paymentHash` — SHA-256, or HASH160 for the
 * `btc_to_arkade` script; {@link preimageMatches} picks by length); a timelock
 * refund path does not.
 */
export function classifyBitcoinSpend(
  witness: string[],
  paymentHash: Uint8Array,
): { spend: "claim"; preimage: Uint8Array } | { spend: "refund" } {
  for (const item of witness) {
    // The preimage is 32 bytes (64 hex chars); skip signatures, pubkeys, scripts.
    if (item.length !== 64) continue;
    const bytes = hex.decode(item);
    if (preimageMatches(bytes, paymentHash))
      return { spend: "claim", preimage: bytes };
  }
  return { spend: "refund" };
}

/**
 * Reduce gathered HTLC facts to an observation (+ the preimage on a claim). A
 * confirmed funding below `expectedSats` is `invalid` (funded, but not on the
 * swap's terms) rather than `confirmed`, so the client never claims a
 * short-funded server leg. An unconfirmed (`mempool`) funding isn't actionable
 * yet, so its amount is re-checked once it confirms.
 */
export function bitcoinObservation(
  facts: BitcoinHtlcFacts,
  paymentHash: Uint8Array,
  expectedSats: number,
): BitcoinHtlcState {
  if (facts.spendWitness) {
    const spend = classifyBitcoinSpend(facts.spendWitness, paymentHash);
    return spend.spend === "claim"
      ? { observation: "spent_claim", preimage: spend.preimage }
      : { observation: "spent_refund" };
  }
  if (facts.funding === "confirmed" && facts.fundedSats < expectedSats) {
    return { observation: "invalid" };
  }
  return { observation: facts.funding };
}
