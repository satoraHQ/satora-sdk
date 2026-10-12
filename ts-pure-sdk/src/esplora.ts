/**
 * Esplora API utilities for Bitcoin transaction lookups.
 *
 * All functions accept either a single Esplora base URL or a list of URLs.
 * With a list, each URL is tried in order and the first successful response
 * wins, so an outage of one explorer doesn't break
 * claims/refunds as long as a fallback instance is reachable.
 */

/** One or more Esplora API base URLs, tried in order. */
export type EsploraUrls = string | string[];

/**
 * Per-request timeouts. Without these a hanging explorer would sit on the
 * platform default (~300s in Node, browser-dependent otherwise), so the
 * fallback URL would never get a chance within the caller's polling
 * budget (e.g. the 30s funding wait in the claim path).
 *
 * Lookups normally return in well under a second, so they fail over
 * quickly; broadcasts get more headroom since the node has to validate
 * and accept the transaction.
 */
const LOOKUP_TIMEOUT_MS = 2_000;
const BROADCAST_TIMEOUT_MS = 10_000;

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
 * `fetch` with a timeout built from `AbortController` + `setTimeout` instead
 * of `AbortSignal.timeout`, which is missing in some supported runtimes
 * (React Native/Hermes). If `AbortController` itself is absent, the request
 * is issued without a timeout rather than throwing before it starts.
 */
async function fetchWithTimeout(
  url: string,
  timeoutMs: number,
  init?: RequestInit,
): Promise<Response> {
  if (typeof AbortController === "undefined") {
    return fetch(url, init);
  }
  const controller = new AbortController();
  const timer = setTimeout(
    () => controller.abort(new Error(`request timed out after ${timeoutMs}ms`)),
    timeoutMs,
  );
  try {
    return await fetch(url, { ...init, signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

/** Normalizes to a trailing-slash-free URL list. */
function toUrlList(esploraUrls: EsploraUrls): string[] {
  const urls = Array.isArray(esploraUrls) ? esploraUrls : [esploraUrls];
  return urls.map((url) => url.replace(/\/+$/, ""));
}

/**
 * Runs `fn` against each URL in order, returning the first successful
 * result. Throws the last error if every URL fails.
 */
async function withFallback<T>(
  esploraUrls: EsploraUrls,
  fn: (url: string) => Promise<T>,
): Promise<T> {
  const urls = toUrlList(esploraUrls);
  if (urls.length === 0) {
    throw new Error("No Esplora URL provided");
  }
  let lastError: unknown;
  for (const url of urls) {
    try {
      return await fn(url);
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError instanceof Error ? lastError : new Error(String(lastError));
}

/**
 * Finds a UTXO at the given address.
 *
 * Queries the Esplora `/address/:address/utxo` endpoint to find
 * unspent outputs. Returns the largest UTXO: the HTLC address is public, so
 * anyone can send extra (dust) outputs to it, and picking by explorer order
 * would let such an output shadow the real deposit — a sub-fee UTXO makes
 * every claim/refund built against it fail. With several genuine deposits,
 * callers recover them largest-first across repeated runs.
 *
 * @param esploraUrls - Esplora API base URL(s), tried in order
 * @param address - The address to look up UTXOs for
 * @returns The txid, vout, and amount of the largest UTXO, or null if none found
 */
export async function findOutputByAddress(
  esploraUrls: EsploraUrls,
  address: string,
): Promise<HtlcOutputResult | null> {
  return withFallback(esploraUrls, async (esploraUrl) => {
    const response = await fetchWithTimeout(
      `${esploraUrl}/address/${address}/utxo`,
      LOOKUP_TIMEOUT_MS,
    );
    if (!response.ok) {
      throw new Error(
        `Failed to fetch UTXOs for address ${address}: ${response.status}`,
      );
    }

    const utxos = (await response.json()) as EsploraUtxo[];

    if (utxos.length === 0) {
      return null;
    }

    const utxo = utxos.reduce((best, candidate) =>
      candidate.value > best.value ? candidate : best,
    );
    return { txid: utxo.txid, vout: utxo.vout, amount: BigInt(utxo.value) };
  });
}

/**
 * Fetches a transaction's outputs via the Esplora `/tx/:txid` endpoint.
 *
 * Returns null when no explorer knows the transaction yet (or all
 * explorers are unreachable) so callers can fall back to an address
 * lookup or keep polling.
 *
 * @param esploraUrls - Esplora API base URL(s), tried in order
 * @param txid - The transaction ID to look up
 */
export async function fetchTransactionOutputs(
  esploraUrls: EsploraUrls,
  txid: string,
): Promise<{ vout: Array<{ value: number }> } | null> {
  try {
    return await withFallback(esploraUrls, async (esploraUrl) => {
      const response = await fetchWithTimeout(
        `${esploraUrl}/tx/${txid}`,
        LOOKUP_TIMEOUT_MS,
      );
      if (!response.ok) {
        throw new Error(`Failed to fetch tx ${txid}: ${response.status}`);
      }
      return (await response.json()) as { vout: Array<{ value: number }> };
    });
  } catch {
    return null;
  }
}

/**
 * Fee rate (sat/vB) the explorer expects to confirm within `targetBlocks`,
 * from the Esplora `/fee-estimates` endpoint. Returns undefined when no
 * explorer answers or none has an estimate for that target (regtest
 * explorers have none).
 *
 * @param esploraUrls - Esplora API base URL(s), tried in order
 * @param targetBlocks - Confirmation target in blocks
 */
export async function fetchFeeRateEstimate(
  esploraUrls: EsploraUrls,
  targetBlocks: number,
): Promise<number | undefined> {
  try {
    return await withFallback(esploraUrls, async (esploraUrl) => {
      const response = await fetchWithTimeout(
        `${esploraUrl}/fee-estimates`,
        LOOKUP_TIMEOUT_MS,
      );
      if (!response.ok) {
        throw new Error(`Failed to fetch fee estimates: ${response.status}`);
      }
      const estimates = (await response.json()) as Record<string, unknown>;
      const rate = estimates[String(targetBlocks)];
      if (typeof rate !== "number" || !Number.isFinite(rate)) {
        throw new Error(`No ${targetBlocks}-block fee estimate`);
      }
      return rate;
    });
  } catch {
    return undefined;
  }
}

/**
 * Broadcasts a raw transaction to the Bitcoin network via Esplora API.
 *
 * With multiple URLs, each is tried in order until one accepts the
 * transaction. If one explorer rejects it with a permanent validation
 * error (e.g. `bad-txns-*`), that error is preferred over transient
 * network errors from other explorers so retry logic doesn't spin on a
 * transaction that can never confirm.
 *
 * @param esploraUrls - Esplora API base URL(s), tried in order
 * @param txHex - The raw transaction hex to broadcast
 * @returns The transaction ID on success
 */
export async function broadcastTransaction(
  esploraUrls: EsploraUrls,
  txHex: string,
): Promise<string> {
  const urls = toUrlList(esploraUrls);
  if (urls.length === 0) {
    throw new Error("No Esplora URL provided");
  }
  const errors: unknown[] = [];
  for (const esploraUrl of urls) {
    try {
      const response = await fetchWithTimeout(
        `${esploraUrl}/tx`,
        BROADCAST_TIMEOUT_MS,
        {
          method: "POST",
          headers: {
            "Content-Type": "text/plain",
          },
          body: txHex,
        },
      );

      if (!response.ok) {
        const errorText = await response.text();
        throw new Error(`Broadcast failed: ${response.status} - ${errorText}`);
      }

      return response.text();
    } catch (error) {
      errors.push(error);
    }
  }
  // Prefer a permanent (non-transient) rejection: it tells the caller the
  // tx itself is invalid, which no amount of retrying or failing over fixes.
  const permanent = errors.find((e) => !isTransientBroadcastError(e));
  const chosen = permanent ?? errors[errors.length - 1];
  throw chosen instanceof Error ? chosen : new Error(String(chosen));
}

/**
 * Heuristic: does this broadcast error look transient (worth retrying)?
 *
 * The common race is that the node we broadcast the claim to has not yet
 * seen the HTLC funding tx, so it rejects the claim's inputs as
 * missing/unknown. Those resolve once the funding tx propagates. Network
 * errors are also treated as transient.
 *
 * Inspects the whole error, not just `.message`: in Node/undici a rejected
 * `fetch` surfaces as `Error("fetch failed")` with the real reason
 * (`ECONNREFUSED`, `ETIMEDOUT`, …) on `error.cause`, so a message-only check
 * would miss connection hiccups and fail fast.
 */
function isTransientBroadcastError(error: unknown): boolean {
  const parts: string[] = [];
  if (error instanceof Error) {
    parts.push(error.message);
    if (error.cause != null) {
      const cause = error.cause;
      parts.push(
        cause instanceof Error
          ? `${cause.message} ${String(cause)}`
          : String(cause),
      );
    }
  } else {
    parts.push(String(error));
  }
  const m = parts.join(" ").toLowerCase();
  return (
    m.includes("missingorspent") ||
    m.includes("missing-inputs") ||
    m.includes("bad-txns-inputs") ||
    m.includes("txn-mempool-conflict") ||
    m.includes("non-bip68-final") ||
    m.includes("no such mempool") ||
    m.includes("not found") ||
    m.includes("failed to fetch") ||
    m.includes("fetch failed") ||
    m.includes("network") ||
    m.includes("timeout") ||
    // Timeout-abort rejections: our fetchWithTimeout reason ("request timed
    // out after Nms"), or a generic AbortError ("the operation was aborted")
    // in runtimes that ignore the abort reason.
    m.includes("timed out") ||
    m.includes("aborted") ||
    m.includes("econnrefused") ||
    m.includes("econnreset") ||
    m.includes("etimedout") ||
    m.includes("eai_again")
  );
}

/**
 * Broadcasts a raw transaction, retrying on transient failures.
 *
 * The funding tx may still be propagating to the broadcast node when the
 * client tries to claim, so the first attempt can fail with a
 * missing-inputs style error that clears within a few seconds. Retries use
 * a capped backoff (500ms → 2s). Each attempt fails over across all
 * configured Esplora URLs.
 *
 * @param esploraUrls - Esplora API base URL(s), tried in order
 * @param txHex - The raw transaction hex to broadcast
 * @param retries - Number of additional attempts after the first (default 5)
 * @returns The transaction ID on success
 */
export async function broadcastTransactionWithRetry(
  esploraUrls: EsploraUrls,
  txHex: string,
  retries = 5,
): Promise<string> {
  let delayMs = 500;
  let lastError: unknown;
  for (let attempt = 0; attempt <= retries; attempt++) {
    try {
      return await broadcastTransaction(esploraUrls, txHex);
    } catch (error) {
      lastError = error;
      // Don't waste attempts on errors that won't clear on their own.
      if (attempt >= retries || !isTransientBroadcastError(error)) {
        throw error;
      }
      await new Promise((resolve) => setTimeout(resolve, delayMs));
      delayMs = Math.min(delayMs * 2, 2_000);
    }
  }
  // Unreachable, but satisfies the type checker.
  throw lastError instanceof Error ? lastError : new Error(String(lastError));
}
