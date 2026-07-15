/**
 * The `ArkadeContractManager`'s pure core.
 *
 * The manager (built on `@arkade-os/sdk`'s ContractManager) gathers facts about a
 * VHTLC — is it funded, and if spent, how — and this maps them to an
 * {@link HtlcObservation} the swap pipeline consumes.
 */
import {
  ConditionWitness,
  getArkPsbtFields,
  Transaction,
} from "@arkade-os/sdk";
import { base64 } from "@scure/base";
import type { HtlcObservation } from "../actions/types.js";
import { preimageMatches } from "./preimage.js";

/** Facts about a VHTLC gathered from the Ark indexer/manager. */
export type ArkadeVtxoFacts = {
  /** A vtxo exists at the VHTLC address (it was funded). */
  funded: boolean;
  /**
   * Whether the funding meets the swap's expected amount. Only consulted when
   * `funded`; `false` → the VHTLC is funded but short → `invalid`, not confirmed.
   */
  sufficient?: boolean;
  /**
   * If the VHTLC output was spent, which path the spending tx took — resolved
   * from the spend tx's `ConditionWitness` (preimage present → `claim`).
   */
  spend?: "claim" | "refund";
};

/**
 * Map gathered VHTLC facts to an on-chain observation.
 *
 * Note: there is no `mempool` here. An Arkade `preconfirmed` vtxo is server-
 * signed (a far stronger trust assumption than a droppable Bitcoin mempool tx),
 * so a funded VHTLC — preconfirmed or settled — is treated as `confirmed`.
 * `mempool` is reserved for Bitcoin on-chain observers.
 */
export function arkadeObservation(facts: ArkadeVtxoFacts): HtlcObservation {
  if (facts.spend === "claim") return "spent_claim";
  if (facts.spend === "refund") return "spent_refund";
  if (!facts.funded) return "absent";
  return facts.sufficient === false ? "invalid" : "confirmed";
}

/** How a VHTLC output was spent. A claim also yields the verified preimage. */
export type ArkadeSpend =
  | { spend: "claim"; preimage: Uint8Array }
  | { spend: "refund" };

/**
 * Classify how a VHTLC output was spent from the spending tx's PSBT.
 *
 * The claim path finalizes the VHTLC's condition-multisig closure, so the spend
 * tx witnesses the preimage in the `ConditionWitness` proprietary field; the
 * unilateral refund path (CSV timelock) never carries it. But we don't trust the
 * field's mere presence — we verify a revealed element actually hashes to *this*
 * swap's `paymentHash` (the SHA-256 preimage hash, i.e. the Lightning
 * payment_hash). That turns "looks like a claim" into "this is the claim of our
 * swap, and here is the verified preimage" — which the caller needs to settle the
 * counterparty leg (the same reason the backend extracts it in
 * `unified_watcher.rs`). Anything else — no field, or a non-matching field — is a
 * refund.
 *
 * Operates on the already-fetched, base64-encoded PSBT so it stays pure and
 * unit-testable; {@link fetchArkadeSpend} does the indexer I/O.
 */
export function classifyArkadeSpend(
  encodedSpendTx: string,
  paymentHash: Uint8Array,
): ArkadeSpend {
  const tx = Transaction.fromPSBT(base64.decode(encodedSpendTx));
  // The VHTLC is always the spending tx's first input; the condition witness is
  // a stack of elements, one of which (on a claim) is the 32-byte preimage.
  for (const witness of getArkPsbtFields(tx, 0, ConditionWitness)) {
    for (const element of witness) {
      if (element.length === 32 && preimageMatches(element, paymentHash)) {
        return { spend: "claim", preimage: element };
      }
    }
  }
  return { spend: "refund" };
}

/** The slice of `@arkade-os/sdk`'s indexer the spend classifier needs. */
export type VirtualTxSource = {
  getVirtualTxs(txids: string[]): Promise<{ txs: string[] }>;
};

/**
 * Fetch the tx that spent a VHTLC and classify its path. Returns `undefined`
 * when the indexer can't yet return the spending tx (e.g. not indexed), so the
 * caller keeps the output as `confirmed` rather than guessing.
 */
export async function fetchArkadeSpend(
  source: VirtualTxSource,
  spentByTxid: string,
  paymentHash: Uint8Array,
): Promise<ArkadeSpend | undefined> {
  const { txs } = await source.getVirtualTxs([spentByTxid]);
  const encoded = txs[0];
  return encoded ? classifyArkadeSpend(encoded, paymentHash) : undefined;
}
