/**
 * A esplora-backed {@link BitcoinChainReader} — the concrete chain source the
 * {@link BitcoinContractManager} uses in production.
 *
 * Kept separate from the manager so the manager stays free of any HTTP/esplora
 * dependency (and unit-testable against a fake reader). A single
 * `GET /address/{addr}/txs` yields both the funding state and the spending
 * input's witness, so no extra round-trips are needed.
 */

import type { BitcoinHtlcFacts } from "./bitcoin.js";
import type { BitcoinChainReader } from "./bitcoin-manager.js";

/** The slice of esplora's tx JSON we read. */
type EsploraTx = {
  vin: Array<{
    witness?: string[];
    prevout?: { scriptpubkey_address?: string } | null;
  }>;
  vout: Array<{ scriptpubkey_address?: string; value?: number }>;
  status?: { confirmed?: boolean; block_height?: number };
};

/**
 * When a funding tx counts as `confirmed` for observation purposes.
 *
 * `minConfirmations: 0` (the default) accepts an UNCONFIRMED funding, so a swap
 * claims as soon as the funding hits the mempool instead of waiting ~10min for
 * a block. This TRUSTS the funder not to double-spend: claiming publishes the
 * preimage, so a funder who then replaces its funding tx could take both legs.
 * Our own server therefore broadcasts HTLC fundings without signalling RBF.
 * Note that a non-signalling tx is still replaceable by full-RBF miners — set
 * `1` (or more) for a policy that doesn't rely on the funder's good behaviour;
 * values above 1 make the reader fetch the tip height to compute depth.
 */
/** A depth already resolved to a number, for the pure classifier below. */
export type BitcoinConfirmationPolicy = {
  minConfirmations?: number;
};

/** A depth, or a getter resolved per read so it can change without a rebuild. */
export type MinConfirmationsSource = number | (() => number | undefined);

export type BitcoinReaderPolicy = {
  minConfirmations?: MinConfirmationsSource;
};

export function resolveMinConfirmations(
  source: MinConfirmationsSource | undefined,
): number {
  return (typeof source === "function" ? source() : source) ?? 0;
}

/**
 * Reduce an address's esplora tx history to HTLC facts. If a tx spends an output
 * at the address, that's the resolving spend (its witness classifies claim vs
 * refund); otherwise a tx paying the address is the funding, gated by the
 * confirmation policy (see {@link BitcoinConfirmationPolicy}).
 */
export function htlcFactsFromEsploraTxs(
  txs: EsploraTx[],
  address: string,
  opts?: BitcoinConfirmationPolicy & { tipHeight?: number },
): BitcoinHtlcFacts {
  for (const tx of txs) {
    const spend = tx.vin.find(
      (vin) => vin.prevout?.scriptpubkey_address === address,
    );
    if (spend)
      return {
        funding: "confirmed",
        fundedSats: 0, // already spent — the amount no longer matters
        spendWitness: spend.witness ?? [],
      };
  }
  // Among all txs paying the address, the one paying the MOST is the funding
  // candidate — matching the pure SDK's UTXO lookup. The address is public,
  // so a stray dust payment must not be mistaken for the funding and observe
  // the swap as underfunded/invalid.
  const paidSats = (tx: EsploraTx): number =>
    tx.vout
      .filter((vout) => vout.scriptpubkey_address === address)
      .reduce((sum, vout) => sum + (vout.value ?? 0), 0);
  let funding: EsploraTx | undefined;
  let fundedSats = 0;
  for (const tx of txs) {
    const paid = paidSats(tx);
    if (paid > fundedSats) {
      funding = tx;
      fundedSats = paid;
    }
  }
  if (funding) {
    const minConf = opts?.minConfirmations ?? 0;
    let deepEnough: boolean;
    if (funding.status?.confirmed) {
      deepEnough =
        minConf <= 1 ||
        (opts?.tipHeight !== undefined &&
          funding.status.block_height !== undefined &&
          opts.tipHeight - funding.status.block_height + 1 >= minConf);
    } else {
      deepEnough = minConf === 0;
    }
    return {
      funding: deepEnough ? "confirmed" : "mempool",
      fundedSats,
    };
  }
  return { funding: "absent", fundedSats: 0 };
}

/**
 * Public esplora endpoints tried in rotation (mainnet). blockstream.info runs the
 * reference esplora, so it's API-compatible with mempool.space; spreading calls
 * across both halves the per-provider load and survives one being throttled.
 */
export const DEFAULT_ESPLORA_URLS = [
  "https://mempool.space/api",
  "https://blockstream.info/api",
];

/**
 * Build a {@link BitcoinChainReader} over one or more esplora REST endpoints.
 * With several, each call starts at a rotating endpoint (to spread load across
 * providers) and fails over to the rest on error — so a throttled or failing
 * provider doesn't stall tracking.
 */
export function esploraReader(
  esploraUrls: string | string[],
  fetchImpl: typeof fetch = fetch,
  policy?: BitcoinReaderPolicy,
): BitcoinChainReader {
  const bases = (Array.isArray(esploraUrls) ? esploraUrls : [esploraUrls]).map(
    (url) => url.replace(/\/+$/, ""),
  );
  let start = 0;
  return {
    async getHtlcFacts(address, minConfirmations) {
      const required =
        minConfirmations ?? resolveMinConfirmations(policy?.minConfirmations);
      const from = start++ % bases.length; // rotate the primary to spread load
      let lastError: unknown;
      for (let i = 0; i < bases.length; i++) {
        const base = bases[(from + i) % bases.length];
        try {
          const res = await fetchImpl(
            `${base}/address/${encodeURIComponent(address)}/txs`,
          );
          if (!res.ok) throw new Error(`esplora ${res.status} at ${base}`);
          const txs = (await res.json()) as EsploraTx[];
          // Depth policies beyond 1 need the tip to compute confirmations;
          // 0 and 1 read straight off the tx's confirmed flag.
          let tipHeight: number | undefined;
          if (required > 1) {
            const tip = await fetchImpl(`${base}/blocks/tip/height`);
            if (!tip.ok) throw new Error(`esplora ${tip.status} at ${base}`);
            tipHeight = Number(await tip.text());
          }
          return htlcFactsFromEsploraTxs(txs, address, {
            minConfirmations: required,
            tipHeight,
          });
        } catch (error) {
          lastError = error;
        }
      }
      throw lastError ?? new Error("no esplora endpoints configured");
    },
  };
}
