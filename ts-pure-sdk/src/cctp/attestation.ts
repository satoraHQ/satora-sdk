/**
 * CCTP attestation client.
 *
 * Polls Circle's IRIS API for attestations after a USDC burn.
 * Used on the destination chain to call MessageTransmitter.receiveMessage().
 */

import {
  CCTP_DOMAINS,
  type CctpChainName,
  IRIS_API_MAINNET,
} from "./constants.js";
import type {
  AttestationResponse,
  CctpMessageResult,
  CctpMessageStatus,
} from "./types.js";

export interface FetchAttestationOptions {
  /** Source chain name (e.g. "Polygon"). */
  sourceChain: CctpChainName;
  /** Transaction hash of the burn on the source chain. */
  txHash: string;
  /** IRIS API base URL. Defaults to mainnet. */
  irisApiUrl?: string;
  /** Polling interval in ms. Defaults to 10000 (10s). */
  pollIntervalMs?: number;
  /** Maximum time to wait in ms. Defaults to 900000 (15min). */
  timeoutMs?: number;
  /** Optional abort signal. */
  signal?: AbortSignal;
}

export interface AttestationResult {
  /** Raw CCTP message bytes (hex with 0x prefix). */
  message: string;
  /** Attestation signature bytes (hex with 0x prefix). */
  attestation: string;
}

/**
 * Poll the IRIS V2 API until the attestation is ready for a given burn tx.
 *
 * @returns The message and attestation bytes needed to call receiveMessage().
 * @throws If the attestation is not ready within the timeout, or the request fails.
 */
export async function fetchAttestation(
  options: FetchAttestationOptions,
): Promise<AttestationResult> {
  const {
    sourceChain,
    txHash,
    irisApiUrl = IRIS_API_MAINNET,
    pollIntervalMs = 10_000,
    timeoutMs = 900_000,
    signal,
  } = options;

  const sourceDomain = CCTP_DOMAINS[sourceChain];
  if (sourceDomain === undefined) {
    throw new Error(`Unknown CCTP source chain: ${sourceChain}`);
  }

  const url = `${irisApiUrl}/v2/messages/${sourceDomain}?transactionHash=${txHash}`;

  const startTime = Date.now();

  while (Date.now() - startTime < timeoutMs) {
    signal?.throwIfAborted();

    try {
      const response = await fetch(url);

      if (response.status === 404) {
        // Message not yet indexed, keep polling
        await sleep(pollIntervalMs, signal);
        continue;
      }

      if (!response.ok) {
        throw new Error(
          `IRIS API error: ${response.status} ${response.statusText}`,
        );
      }

      const data: AttestationResponse = await response.json();

      if (data.messages.length > 0 && data.messages[0].status === "complete") {
        return {
          message: data.messages[0].message,
          attestation: data.messages[0].attestation,
        };
      }

      // Attestation pending, keep polling
      await sleep(pollIntervalMs, signal);
    } catch (err) {
      if (err instanceof DOMException && err.name === "AbortError") {
        throw err;
      }
      if (err instanceof CctpForwardingFailedError) {
        // Terminal state from IRIS — retrying cannot change it.
        throw err;
      }
      // Network errors — retry after delay
      await sleep(pollIntervalMs, signal);
    }
  }

  throw new Error(
    `Attestation not ready after ${timeoutMs / 1000}s for tx ${txHash} on ${sourceChain}`,
  );
}

/**
 * Whether IRIS has indexed a CCTP message for a burn tx, attested or not.
 * A single lookup, unlike {@link fetchAttestation}.
 *
 * @throws If IRIS cannot be reached or answers with an error, so an outage
 *   is never mistaken for an unknown burn.
 */
export async function hasCctpMessage(options: {
  sourceChain: CctpChainName;
  txHash: string;
  irisApiUrl?: string;
}): Promise<boolean> {
  const { sourceChain, txHash, irisApiUrl = IRIS_API_MAINNET } = options;
  const sourceDomain = CCTP_DOMAINS[sourceChain];
  if (sourceDomain === undefined) {
    throw new Error(`Unknown CCTP source chain: ${sourceChain}`);
  }
  const response = await fetch(
    `${irisApiUrl}/v2/messages/${sourceDomain}?transactionHash=${txHash}`,
  );
  if (response.status === 404) return false;
  if (!response.ok) {
    throw new Error(
      `IRIS API error: ${response.status} ${response.statusText}`,
    );
  }
  const data: AttestationResponse = await response.json();
  return data.messages.length > 0;
}

// ============================================================================
// Cross-chain message tracking (with forwarding)
// ============================================================================

/** A CCTP message reached a terminal non-delivered forwarding state. */
export class CctpForwardingFailedError extends Error {}

/** Options for tracking a CCTP cross-chain message via IRIS. */
export interface TrackCctpMessageOptions {
  /** Source chain name (e.g. "Arbitrum"). */
  sourceChain: CctpChainName;
  /** Transaction hash of the burn/claim on the source chain. */
  txHash: string;
  /** IRIS API base URL. Defaults to mainnet. */
  irisApiUrl?: string;
  /** Polling interval in ms. Defaults to 5000 (5s). */
  pollIntervalMs?: number;
  /** Maximum time to wait in ms. Defaults to 600000 (10min). */
  timeoutMs?: number;
  /** Optional abort signal. */
  signal?: AbortSignal;
  /** Optional callback invoked when the message status changes. */
  onStatusChange?: (status: CctpMessageStatus) => void;
}

/**
 * Poll the IRIS V2 API until the CCTP message is forwarded and delivered.
 *
 * Tracks attestation → forwarding → delivery on the destination chain.
 *
 * @returns The message result with forwarding details (destination tx hash, amount, fee).
 * @throws If the message is not delivered within the timeout.
 */
export async function trackCctpMessage(
  options: TrackCctpMessageOptions,
): Promise<CctpMessageResult> {
  const {
    sourceChain,
    txHash,
    irisApiUrl = IRIS_API_MAINNET,
    pollIntervalMs = 5_000,
    timeoutMs = 600_000,
    signal,
    onStatusChange,
  } = options;

  const sourceDomain = CCTP_DOMAINS[sourceChain];
  if (sourceDomain === undefined) {
    throw new Error(`Unknown CCTP source chain: ${sourceChain}`);
  }

  const url = `${irisApiUrl}/v2/messages/${sourceDomain}?transactionHash=${txHash}`;
  const startTime = Date.now();
  let lastStatus: CctpMessageStatus | undefined;

  const emitStatus = (status: CctpMessageStatus) => {
    if (status !== lastStatus) {
      lastStatus = status;
      onStatusChange?.(status);
    }
  };

  while (Date.now() - startTime < timeoutMs) {
    signal?.throwIfAborted();

    try {
      const response = await fetch(url);

      if (response.status === 404) {
        emitStatus("PENDING");
        await sleep(pollIntervalMs, signal);
        continue;
      }

      if (!response.ok) {
        throw new Error(
          `IRIS API error: ${response.status} ${response.statusText}`,
        );
      }

      const data: AttestationResponse = await response.json();

      if (data.messages.length === 0) {
        emitStatus("PENDING");
        await sleep(pollIntervalMs, signal);
        continue;
      }

      const msg = data.messages[0];

      if (msg.status !== "complete") {
        emitStatus("CONFIRMING");
        await sleep(pollIntervalMs, signal);
        continue;
      }

      // Attestation is complete — check forwarding state
      if (
        (msg.forwardState === "COMPLETE" || msg.forwardState === "CONFIRMED") &&
        msg.forwardTxHash
      ) {
        emitStatus("COMPLETE");
        const cctpSentAmount = msg.decodedMessage?.decodedMessageBody?.amount;
        const cctpPaidFee = msg.decodedMessage?.decodedMessageBody?.feeExecuted;
        return {
          status: "COMPLETE",
          forwardTxHash: msg.forwardTxHash,
          amount:
            cctpSentAmount && cctpPaidFee
              ? (BigInt(cctpSentAmount) - BigInt(cctpPaidFee)).toString()
              : undefined,
          feeExecuted: msg.decodedMessage?.decodedMessageBody?.feeExecuted,
        };
      }

      // Forwarding failed — stop polling
      if (
        msg.forwardState &&
        msg.forwardState !== "PENDING" &&
        msg.forwardState !== "COMPLETE"
      ) {
        emitStatus("FAILED");
        throw new CctpForwardingFailedError(
          `CCTP forwarding failed (state: ${msg.forwardState}) for tx ${txHash} on ${sourceChain}`,
        );
      }

      // Attestation complete but forwarding still in progress
      emitStatus("FORWARDING");
      await sleep(pollIntervalMs, signal);
    } catch (err) {
      if (err instanceof DOMException && err.name === "AbortError") {
        throw err;
      }
      if (err instanceof CctpForwardingFailedError) {
        // Terminal state from IRIS — retrying cannot change it.
        throw err;
      }
      // Network errors — retry after delay
      await sleep(pollIntervalMs, signal);
    }
  }

  throw new Error(
    `CCTP message not delivered after ${timeoutMs / 1000}s for tx ${txHash} on ${sourceChain}`,
  );
}

// ============================================================================
// Helpers
// ============================================================================

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
