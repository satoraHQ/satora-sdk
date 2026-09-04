/**
 * An Electrum-backed {@link BitcoinChainReader} — reads HTLC facts from our own
 * Fulcrum over WebSocket instead of public esplora REST endpoints.
 *
 * Two advantages over the esplora reader:
 *
 * - **Freshness**: our Fulcrum sees a transaction the moment its bitcoind
 *   accepts it, while public explorers index with a lag — the exact lag that
 *   made a server-funding hint's targeted verify come back "absent" and park
 *   the claim on the 60s at-risk poller.
 * - **Push**: implements the reader's optional `subscribe` capability via
 *   `blockchain.scripthash.subscribe`, so the manager reconciles the moment
 *   an HTLC address's history changes instead of waiting for a poll tick.
 *
 * Reads fall back to a wrapped reader (typically the esplora one) whenever the
 * Electrum server errors, so a Fulcrum outage degrades to today's behavior
 * rather than blinding the tracker.
 */

import { sha256 } from "@noble/hashes/sha2.js";
import { hex } from "@scure/base";
import * as btc from "@scure/btc-signer";
import type { BitcoinHtlcFacts } from "./bitcoin.js";
import type { BitcoinChainReader } from "./bitcoin-manager.js";
import {
  type BitcoinReaderPolicy,
  resolveMinConfirmations,
} from "./bitcoin-reader-esplora.js";

/** One entry of `blockchain.scripthash.get_history`. */
type ElectrumHistoryEntry = {
  tx_hash: string;
  /** Block height; 0 for mempool, -1 for mempool with unconfirmed parents. */
  height: number;
};

/**
 * The Electrum client surface this reader needs. The pure SDK's
 * `ElectrumWsClient` satisfies it; tests inject a fake.
 */
export type ElectrumRpc = {
  request<T>(method: string, params: unknown[]): Promise<T>;
  subscribeScriptHash(
    scripthash: string,
    onChange: (status: string | null) => void,
  ): Promise<string | null>;
  unsubscribeScriptHash(
    scripthash: string,
    onChange: (status: string | null) => void,
  ): void;
};

export type ElectrumReaderNetwork =
  | "mainnet"
  | "testnet"
  | "signet"
  | "regtest";

/** btc-signer only ships mainnet/testnet; regtest differs in bech32 prefix. */
const REGTEST_NETWORK = {
  bech32: "bcrt",
  pubKeyHash: 0x6f,
  scriptHash: 0xc4,
  wif: 0xef,
} as const;

function toBtcSignerNetwork(network: ElectrumReaderNetwork) {
  switch (network) {
    case "mainnet":
      return btc.NETWORK;
    case "testnet":
    case "signet":
      return btc.TEST_NETWORK;
    case "regtest":
      return REGTEST_NETWORK;
  }
}

/** Electrum scripthash: sha256 of the output script, reversed, hex. */
function scriptHashOf(script: Uint8Array): string {
  return hex.encode(sha256(script).reverse());
}

/**
 * Build a {@link BitcoinChainReader} over an Electrum connection, with push
 * support and an optional fallback reader for outages.
 */
export function electrumReader(
  client: ElectrumRpc,
  opts?: BitcoinReaderPolicy & {
    /** Network the HTLC addresses live on; defaults to mainnet. */
    network?: ElectrumReaderNetwork;
    /** Reader consulted when the Electrum server errors (e.g. esplora). */
    fallback?: BitcoinChainReader;
  },
): BitcoinChainReader {
  const network = opts?.network ?? "mainnet";
  const fallback = opts?.fallback;

  const scriptFor = (address: string): Uint8Array =>
    btc.OutScript.encode(
      btc.Address(toBtcSignerNetwork(network)).decode(address),
    );

  async function getHtlcFactsElectrum(
    address: string,
    required: number,
  ): Promise<BitcoinHtlcFacts> {
    const script = scriptFor(address);
    const scripthash = scriptHashOf(script);
    const history = await client.request<ElectrumHistoryEntry[]>(
      "blockchain.scripthash.get_history",
      [scripthash],
    );
    if (!history || history.length === 0)
      return { funding: "absent", fundedSats: 0 };

    // HTLC addresses see at most a handful of txs (fund + resolve), so
    // fetching each raw tx is cheap.
    const txs = await Promise.all(
      history.map(async (entry) => {
        const raw = await client.request<string>("blockchain.transaction.get", [
          entry.tx_hash,
        ]);
        return {
          entry,
          tx: btc.Transaction.fromRaw(hex.decode(raw), {
            allowUnknownOutputs: true,
            allowUnknownInputs: true,
            disableScriptCheck: true,
          }),
        };
      }),
    );

    const scriptHex = hex.encode(script);
    const outputIndicesPaying = (tx: btc.Transaction): number[] => {
      const indices: number[] = [];
      for (let i = 0; i < tx.outputsLength; i++) {
        const out = tx.getOutput(i);
        if (out.script && hex.encode(out.script) === scriptHex) indices.push(i);
      }
      return indices;
    };

    // Among all txs paying the address, the one paying the MOST is the
    // funding candidate — same rule as the pure SDK's UTXO lookup. The
    // address is public, so a stray dust payment (whether it landed before
    // or after the real funding) must not be mistaken for the funding and
    // observe the swap as underfunded/invalid.
    let funding: (typeof txs)[number] | undefined;
    let fundedSats = 0;
    for (const candidate of txs) {
      let paid = 0;
      for (const i of outputIndicesPaying(candidate.tx))
        paid += Number(candidate.tx.getOutput(i).amount ?? 0n);
      if (paid > fundedSats) {
        funding = candidate;
        fundedSats = paid;
      }
    }
    if (!funding) return { funding: "absent", fundedSats: 0 };
    const fundedVouts = outputIndicesPaying(funding.tx);

    // A tx spending one of the funding outputs is the resolving spend; its
    // witness classifies claim vs refund.
    for (const { tx } of txs) {
      for (let i = 0; i < tx.inputsLength; i++) {
        const input = tx.getInput(i);
        if (
          input.txid &&
          hex.encode(input.txid) === funding.entry.tx_hash &&
          input.index !== undefined &&
          fundedVouts.includes(input.index)
        ) {
          return {
            funding: "confirmed",
            fundedSats: 0, // already spent — the amount no longer matters
            spendWitness: (input.finalScriptWitness ?? []).map((item) =>
              hex.encode(item),
            ),
          };
        }
      }
    }

    // Same depth policy as the esplora reader: 0/1 read straight off the
    // confirmed flag; deeper policies need the tip to compute depth.
    const height = funding.entry.height;
    const isConfirmed = height > 0;
    let deepEnough: boolean;
    if (isConfirmed) {
      if (required <= 1) {
        deepEnough = true;
      } else {
        const tip = await client.request<{ height: number }>(
          "blockchain.headers.subscribe",
          [],
        );
        deepEnough = tip.height - height + 1 >= required;
      }
    } else {
      deepEnough = required === 0;
    }
    return { funding: deepEnough ? "confirmed" : "mempool", fundedSats };
  }

  return {
    async getHtlcFacts(address, minConfirmations) {
      const required =
        minConfirmations ?? resolveMinConfirmations(opts?.minConfirmations);
      try {
        return await getHtlcFactsElectrum(address, required);
      } catch (error) {
        if (!fallback) throw error;
        console.warn(
          "electrumReader: falling back to secondary reader:",
          error instanceof Error ? error.message : error,
        );
        return fallback.getHtlcFacts(address, minConfirmations);
      }
    },

    subscribe(address, onChange) {
      // An address the configured network can't decode (e.g. a signet address
      // under a mainnet-configured reader) must not throw out of register()
      // and abort tracking — degrade to the poll cadence instead.
      let scripthash: string;
      try {
        scripthash = scriptHashOf(scriptFor(address));
      } catch (error) {
        console.warn(
          "electrumReader: subscribe skipped (address decode failed):",
          error instanceof Error ? error.message : error,
        );
        return () => {};
      }
      const handler = () => onChange();
      // Best-effort: if the subscription fails the tracker's poll cadence
      // remains the backstop, so the error is logged, not thrown.
      client.subscribeScriptHash(scripthash, handler).catch((error) => {
        console.warn(
          "electrumReader: scripthash subscribe failed:",
          error instanceof Error ? error.message : error,
        );
      });
      return () => client.unsubscribeScriptHash(scripthash, handler);
    },
  };
}
