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
  status?: { confirmed?: boolean };
};

/**
 * Reduce an address's esplora tx history to HTLC facts. If a tx spends an output
 * at the address, that's the resolving spend (its witness classifies claim vs
 * refund); otherwise a tx paying the address is the funding.
 */
export function htlcFactsFromEsploraTxs(
  txs: EsploraTx[],
  address: string,
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
  const funding = txs.find((tx) =>
    tx.vout.some((vout) => vout.scriptpubkey_address === address),
  );
  if (funding) {
    const fundedSats = funding.vout
      .filter((vout) => vout.scriptpubkey_address === address)
      .reduce((sum, vout) => sum + (vout.value ?? 0), 0);
    return {
      funding: funding.status?.confirmed ? "confirmed" : "mempool",
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
): BitcoinChainReader {
  const bases = (Array.isArray(esploraUrls) ? esploraUrls : [esploraUrls]).map(
    (url) => url.replace(/\/+$/, ""),
  );
  let start = 0;
  return {
    async getHtlcFacts(address) {
      const from = start++ % bases.length; // rotate the primary to spread load
      let lastError: unknown;
      for (let i = 0; i < bases.length; i++) {
        const base = bases[(from + i) % bases.length];
        try {
          const res = await fetchImpl(
            `${base}/address/${encodeURIComponent(address)}/txs`,
          );
          if (!res.ok) throw new Error(`esplora ${res.status} at ${base}`);
          return htlcFactsFromEsploraTxs(
            (await res.json()) as EsploraTx[],
            address,
          );
        } catch (error) {
          lastError = error;
        }
      }
      throw lastError ?? new Error("no esplora endpoints configured");
    },
  };
}
