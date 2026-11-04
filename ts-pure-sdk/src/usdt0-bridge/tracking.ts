/**
 * LayerZero message tracking for USDT0 cross-chain transfers.
 *
 * Polls the LayerZero Scan API until the cross-chain message is delivered.
 * Similar pattern to CCTP attestation polling.
 *
 * API: https://scan.layerzero-api.com/v1/messages/tx/{txHash}
 */

// ============================================================================
// Types
// ============================================================================

/** LayerZero message delivery status. */
export type LayerZeroMessageStatus =
  | "INFLIGHT"
  | "CONFIRMING"
  | "DELIVERED"
  | "FAILED"
  | "BLOCKED"
  | "PAYLOAD_STORED";

/** Summary of a tracked LayerZero cross-chain message. */
export interface LayerZeroMessageResult {
  /** Unique cross-chain message identifier. */
  guid: string;
  /** Current delivery status. */
  status: LayerZeroMessageStatus;
  /** Source chain transaction hash. */
  srcTxHash: string;
  /** Destination chain transaction hash (available once delivered). */
  dstTxHash?: string;
  /** Source chain name (e.g. "arbitrum"). */
  srcChain: string;
  /** Destination chain name (e.g. "optimism"). */
  dstChain: string;
  /** Source LayerZero endpoint ID. */
  srcEid: number;
  /** Destination LayerZero endpoint ID. */
  dstEid: number;
}

/** Raw response from the LayerZero Scan API. */
interface ScanApiResponse {
  data: Array<{
    pathway: {
      srcEid: number;
      dstEid: number;
      sender: { chain: string };
      receiver: { chain: string };
    };
    source: {
      status: string;
      tx: { txHash: string };
    };
    destination: {
      status: string;
      tx?: { txHash: string };
    };
    status: {
      name: LayerZeroMessageStatus;
      message: string;
    };
    guid: string;
  }>;
}

// ============================================================================
// Tracking
// ============================================================================

export interface TrackMessageOptions {
  /** Transaction hash of the OFT send on the source chain. */
  txHash: string;
  /** Select the destination message when a transaction emits several messages. */
  destinationEid?: number;
  /** LayerZero Scan API base URL. Defaults to mainnet. */
  apiUrl?: string;
  /** Polling interval in ms. Defaults to 5000 (5s). */
  pollIntervalMs?: number;
  /** Maximum time to wait in ms. Defaults to 300000 (5min). */
  timeoutMs?: number;
  /** Optional abort signal. */
  signal?: AbortSignal;
  /** Optional callback invoked on each poll with the current status. */
  onStatusChange?: (status: LayerZeroMessageStatus) => void;
}

const LZ_SCAN_API = "https://scan.layerzero-api.com";

/**
 * Poll the LayerZero Scan API until the cross-chain message is delivered.
 *
 * @returns The message result with delivery details.
 * @throws If the message is not delivered within the timeout, or status is FAILED.
 */
export async function trackMessage(
  options: TrackMessageOptions,
): Promise<LayerZeroMessageResult> {
  const {
    txHash,
    destinationEid,
    apiUrl = LZ_SCAN_API,
    pollIntervalMs = 5_000,
    timeoutMs = 300_000,
    signal,
    onStatusChange,
  } = options;

  const url = `${apiUrl}/v1/messages/tx/${txHash}`;
  const startTime = Date.now();
  let lastStatus: LayerZeroMessageStatus | undefined;

  while (Date.now() - startTime < timeoutMs) {
    signal?.throwIfAborted();

    try {
      const response = await fetch(url);

      if (response.status === 404 || response.status === 429) {
        // Not yet indexed or rate-limited, keep polling
        await sleep(pollIntervalMs, signal);
        continue;
      }

      if (!response.ok) {
        throw new Error(
          `LayerZero Scan API error: ${response.status} ${response.statusText}`,
        );
      }

      const data: ScanApiResponse = await response.json();

      if (data.data.length === 0) {
        // Not yet indexed
        await sleep(pollIntervalMs, signal);
        continue;
      }

      const msg =
        destinationEid === undefined
          ? data.data[0]
          : data.data.find(
              (message) => message.pathway.dstEid === destinationEid,
            );
      if (!msg) {
        await sleep(pollIntervalMs, signal);
        continue;
      }
      const status = msg.status.name;

      if (status !== lastStatus) {
        lastStatus = status;
        onStatusChange?.(status);
      }

      if (status === "FAILED" || status === "BLOCKED") {
        throw new Error(
          `LayerZero message ${status}: ${msg.status.message} (guid: ${msg.guid})`,
        );
      }

      if (status === "DELIVERED") {
        return {
          guid: msg.guid,
          status,
          srcTxHash: msg.source.tx.txHash,
          dstTxHash: msg.destination.tx?.txHash,
          srcChain: msg.pathway.sender.chain,
          dstChain: msg.pathway.receiver.chain,
          srcEid: msg.pathway.srcEid,
          dstEid: msg.pathway.dstEid,
        };
      }

      // INFLIGHT / CONFIRMING — keep polling
      await sleep(pollIntervalMs, signal);
    } catch (err) {
      if (err instanceof DOMException && err.name === "AbortError") {
        throw err;
      }
      if (
        err instanceof Error &&
        (err.message.includes("FAILED") || err.message.includes("BLOCKED"))
      ) {
        throw err;
      }
      // Network errors — retry after delay
      await sleep(pollIntervalMs, signal);
    }
  }

  throw new Error(
    `LayerZero message not delivered after ${timeoutMs / 1000}s for tx ${txHash}`,
  );
}

/**
 * Get the current status of a LayerZero cross-chain message without polling.
 *
 * @returns The message result, or null if not yet indexed.
 */
export async function getMessageStatus(
  txHash: string,
  apiUrl: string = LZ_SCAN_API,
  destinationEid?: number,
): Promise<LayerZeroMessageResult | null> {
  const url = `${apiUrl}/v1/messages/tx/${txHash}`;
  const response = await fetch(url);

  if (response.status === 404) return null;
  if (!response.ok) {
    throw new Error(
      `LayerZero Scan API error: ${response.status} ${response.statusText}`,
    );
  }

  const data: ScanApiResponse = await response.json();
  if (data.data.length === 0) return null;

  const msg =
    destinationEid === undefined
      ? data.data[0]
      : data.data.find((message) => message.pathway.dstEid === destinationEid);
  if (!msg) return null;
  return {
    guid: msg.guid,
    status: msg.status.name,
    srcTxHash: msg.source.tx.txHash,
    dstTxHash: msg.destination.tx?.txHash,
    srcChain: msg.pathway.sender.chain,
    dstChain: msg.pathway.receiver.chain,
    srcEid: msg.pathway.srcEid,
    dstEid: msg.pathway.dstEid,
  };
}

// ============================================================================
// Helpers
// ============================================================================

/**
 * Get the LayerZero Scan explorer URL for a transaction.
 */
export function getExplorerUrl(txHash: string): string {
  return `https://layerzeroscan.com/tx/${txHash}`;
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener(
      "abort",
      () => {
        clearTimeout(timer);
        reject(new DOMException("Aborted", "AbortError"));
      },
      { once: true },
    );
  });
}
