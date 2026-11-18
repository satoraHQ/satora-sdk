/**
 * Pure classification of a withdrawal destination string — which rail it
 * targets, and (for Lightning) how to map it to the swap SDK's fields. No
 * network calls; the swap backend resolves LNURL / Lightning addresses.
 */

// BOLT11 on any network: bc / tb / tbs / bcrt / sb.
export const isBolt11 = (s: string): boolean =>
  /^ln(bcrt|bc|tbs|tb|sb)/i.test(s);
export const isLnurl = (s: string): boolean =>
  /^lnurl1[0-9ac-hj-np-z]+$/i.test(s);
export const isLnAddress = (s: string): boolean =>
  /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(s);

/** A Lightning destination: a BOLT11 invoice, an LNURL, or a Lightning address. */
export function isLightningDestination(s: string): boolean {
  const input = s.trim();
  return isBolt11(input) || isLnurl(input) || isLnAddress(input);
}

/** An Arkade address (bech32m, hrp `ark` mainnet / `tark` test networks). */
export function isArkadeAddress(s: string): boolean {
  return /^t?ark1[0-9ac-hj-np-z]+$/i.test(s.trim());
}

/** Which withdrawal rail a destination string targets. */
export type DestinationKind = "lightning" | "arkade" | "l1";

/**
 * Classify a withdrawal destination. Lightning (invoice/LNURL/address) and
 * Arkade (`ark1…`/`tark1…`) are matched explicitly; everything else is treated
 * as an onchain Bitcoin (L1) address.
 */
export function classifyDestination(destination: string): DestinationKind {
  const input = destination.trim();
  if (isLightningDestination(input)) return "lightning";
  if (isArkadeAddress(input)) return "arkade";
  return "l1";
}

/** Fields of the swap SDK's Arkade→Lightning options we route a destination to. */
export type LightningDestination =
  | {
      lightningInvoice: string;
      /**
       * Fixed lock: exact sats to lock from the payout. Must cover the invoice
       * amount + protocol fee + Lightning send fee; any surplus is charged as
       * network fee (capped by the backend).
       */
      sourceAmountSats?: number;
    }
  | {
      lightningAddress: string;
      /** Sats the recipient receives (a BOLT11 invoice pins its own). */
      targetAmountSats: number;
    }
  | {
      lnurl: string;
      /** Sats the recipient receives (a BOLT11 invoice pins its own). */
      targetAmountSats: number;
    };

/**
 * Map a Lightning destination string to the swap SDK's mutually-exclusive
 * `lightningInvoice` / `lightningAddress` / `lnurl` fields. Throws if it isn't a
 * Lightning destination, or if `amountSats` is missing for LNURL / address.
 *
 * `sourceAmountSats` (BOLT11 only) is a fixed lock: the exact sats to lock from
 * the payout, passed through as the swap SDK's `sourceAmountSats`. It must be a
 * positive integer, and is rejected for LNURL / address, whose amount is the
 * recipient amount (`amountSats`).
 */
export function toLightningDestination(
  destination: string,
  amountSats?: number,
  sourceAmountSats?: number,
): LightningDestination {
  const input = destination.trim();
  if (
    sourceAmountSats !== undefined &&
    (!Number.isSafeInteger(sourceAmountSats) || sourceAmountSats <= 0)
  ) {
    throw new Error(
      `sourceAmountSats must be a positive integer, got ${sourceAmountSats}`,
    );
  }
  // The BOLT11 invoice carries its own amount, so amountSats is not needed.
  if (isBolt11(input)) {
    return sourceAmountSats === undefined
      ? { lightningInvoice: input }
      : { lightningInvoice: input, sourceAmountSats };
  }
  const address = isLnAddress(input);
  const lnurl = isLnurl(input);
  if (!address && !lnurl) {
    throw new Error(
      "unrecognized Lightning destination: expected a BOLT11 invoice, " +
        "LNURL (lnurl1...), or Lightning address (user@host)",
    );
  }
  if (sourceAmountSats !== undefined) {
    throw new Error(
      "sourceAmountSats is only supported with a BOLT11 invoice; pass " +
        `amountSats (the recipient amount) for a Lightning ${address ? "address" : "LNURL"}`,
    );
  }
  if (amountSats === undefined) {
    throw new Error(
      `amountSats is required for a Lightning ${address ? "address" : "LNURL"} destination`,
    );
  }
  return address
    ? { lightningAddress: input, targetAmountSats: amountSats }
    : { lnurl: input, targetAmountSats: amountSats };
}
