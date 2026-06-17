/**
 * Esplora API utilities for Bitcoin transaction lookups.
 */

/** Esplora UTXO response */
export interface EsploraUtxo {
  txid: string;
  vout: number;
  status: {
    confirmed: boolean;
    block_height?: number;
    block_hash?: string;
    block_time?: number;
  };
  value: number;
}

/** Result of finding an HTLC output */
export interface HtlcOutputResult {
  txid: string;
  vout: number;
  amount: bigint;
}

/**
 * Finds a UTXO at the given address.
 *
 * Queries the Esplora `/address/:address/utxo` endpoint to find
 * unspent outputs. Returns the first UTXO found.
 *
 * @param esploraUrl - The Esplora API base URL
 * @param address - The address to look up UTXOs for
 * @returns The txid, vout, and amount of the first UTXO, or null if none found
 */
export async function findOutputByAddress(
  esploraUrl: string,
  address: string,
): Promise<HtlcOutputResult | null> {
  const response = await fetch(`${esploraUrl}/address/${address}/utxo`);
  if (!response.ok) {
    throw new Error(
      `Failed to fetch UTXOs for address ${address}: ${response.status}`,
    );
  }

  const utxos = (await response.json()) as EsploraUtxo[];

  if (utxos.length === 0) {
    return null;
  }

  const utxo = utxos[0];
  return { txid: utxo.txid, vout: utxo.vout, amount: BigInt(utxo.value) };
}

/**
 * Broadcasts a raw transaction to the Bitcoin network via Esplora API.
 *
 * @param esploraUrl - The Esplora API base URL
 * @param txHex - The raw transaction hex to broadcast
 * @returns The transaction ID on success
 */
export async function broadcastTransaction(
  esploraUrl: string,
  txHex: string,
): Promise<string> {
  const response = await fetch(`${esploraUrl}/tx`, {
    method: "POST",
    headers: {
      "Content-Type": "text/plain",
    },
    body: txHex,
  });

  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(`Broadcast failed: ${response.status} - ${errorText}`);
  }

  return response.text();
}

/**
 * Heuristic: does this broadcast error look transient (worth retrying)?
 *
 * The common race is that the node we broadcast the claim to has not yet
 * seen the HTLC funding tx, so it rejects the claim's inputs as
 * missing/unknown. Those resolve once the funding tx propagates. Network
 * errors are also treated as transient.
 */
function isTransientBroadcastError(message: string): boolean {
  const m = message.toLowerCase();
  return (
    m.includes("missingorspent") ||
    m.includes("missing-inputs") ||
    m.includes("bad-txns-inputs") ||
    m.includes("txn-mempool-conflict") ||
    m.includes("non-bip68-final") ||
    m.includes("no such mempool") ||
    m.includes("not found") ||
    m.includes("failed to fetch") ||
    m.includes("network") ||
    m.includes("timeout") ||
    m.includes("econnrefused")
  );
}

/**
 * Broadcasts a raw transaction, retrying on transient failures.
 *
 * The funding tx may still be propagating to the broadcast node when the
 * client tries to claim, so the first attempt can fail with a
 * missing-inputs style error that clears within a few seconds. Retries use
 * a capped backoff (500ms → 2s).
 *
 * @param esploraUrl - The Esplora API base URL
 * @param txHex - The raw transaction hex to broadcast
 * @param retries - Number of additional attempts after the first (default 5)
 * @returns The transaction ID on success
 */
export async function broadcastTransactionWithRetry(
  esploraUrl: string,
  txHex: string,
  retries = 5,
): Promise<string> {
  let delayMs = 500;
  let lastError: unknown;
  for (let attempt = 0; attempt <= retries; attempt++) {
    try {
      return await broadcastTransaction(esploraUrl, txHex);
    } catch (error) {
      lastError = error;
      const msg = error instanceof Error ? error.message : String(error);
      // Don't waste attempts on errors that won't clear on their own.
      if (attempt >= retries || !isTransientBroadcastError(msg)) {
        throw error;
      }
      await new Promise((resolve) => setTimeout(resolve, delayMs));
      delayMs = Math.min(delayMs * 2, 2_000);
    }
  }
  // Unreachable, but satisfies the type checker.
  throw lastError instanceof Error ? lastError : new Error(String(lastError));
}
