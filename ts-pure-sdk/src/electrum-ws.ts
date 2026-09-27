/**
 * Electrum protocol client over WebSocket (Fulcrum-compatible).
 *
 * Fulcrum serves the same JSON-RPC protocol on its `ws`/`wss` ports as on
 * TCP/SSL, so a browser (which cannot open raw TCP sockets) can talk to it
 * directly. This gives the SDK two things Esplora polling cannot:
 *
 * - Lookups against our own infrastructure instead of rate-limited public
 *   explorers.
 * - `blockchain.scripthash.subscribe` push notifications, so waiting for an
 *   HTLC funding output resolves the moment the server sees the tx instead
 *   of on the next poll tick.
 *
 * The client is lazy: no socket is opened until the first request. While
 * subscriptions are active it reconnects with capped backoff and re-issues
 * the subscriptions; with no subscriptions a dropped socket simply
 * reconnects on the next request.
 */

import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import * as btc from "@scure/btc-signer";

/** Networks the SDK deals in (mirrors `refund/onchain.ts`). */
export type ElectrumNetwork = "mainnet" | "testnet" | "signet" | "regtest";

/** One entry of `blockchain.scripthash.listunspent`. */
export interface ElectrumUnspent {
  tx_hash: string;
  tx_pos: number;
  /** Block height; 0 for mempool, -1 for mempool with unconfirmed parents. */
  height: number;
  /** Value in satoshis. */
  value: number;
}

/** Minimal logging hook so the SDK's logger can observe this module. */
export type ElectrumLogHook = (
  level: "debug" | "warn",
  event: string,
  message: string,
  data?: Record<string, unknown>,
) => void;

const REQUEST_TIMEOUT_MS = 5_000;
const BROADCAST_TIMEOUT_MS = 10_000;
const CONNECT_TIMEOUT_MS = 8_000;
const PING_INTERVAL_MS = 60_000;
const MIN_RECONNECT_MS = 1_000;
const MAX_RECONNECT_MS = 30_000;
/**
 * With no in-flight requests and no subscriptions the socket is closed after
 * this long, so a Node process (e2e script, CLI) isn't kept alive by an idle
 * connection. The next request reconnects transparently.
 */
const IDLE_CLOSE_MS = 30_000;

/** btc-signer only ships mainnet/testnet; regtest differs in bech32 prefix. */
const REGTEST_NETWORK = {
  bech32: "bcrt",
  pubKeyHash: 0x6f,
  scriptHash: 0xc4,
  wif: 0xef,
} as const;

function toBtcSignerNetwork(network: ElectrumNetwork) {
  switch (network) {
    case "mainnet":
      return btc.NETWORK;
    case "testnet":
    case "signet":
      return btc.TEST_NETWORK;
    case "regtest":
      return REGTEST_NETWORK;
    default:
      throw new Error(`Unknown network: ${network}`);
  }
}

/**
 * Electrum scripthash for an address: sha256 of the output script,
 * hex-encoded in reversed byte order.
 */
export function addressToScriptHash(
  address: string,
  network: ElectrumNetwork,
): string {
  const net = toBtcSignerNetwork(network);
  const decoded = btc.Address(net).decode(address);
  if (!decoded) throw new Error(`Unrecognised Bitcoin address: ${address}`);
  const script = btc.OutScript.encode(decoded);
  return bytesToHex(sha256(script).reverse());
}

interface PendingRequest {
  resolve: (result: unknown) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

type ScriptHashHandler = (status: string | null) => void;

/**
 * A single-socket Electrum JSON-RPC client over WebSocket.
 *
 * All calls are `id`-correlated; `blockchain.scripthash.subscribe`
 * notifications are dispatched to registered handlers.
 */
export class ElectrumWsClient {
  readonly #url: string;
  readonly #log: ElectrumLogHook;

  #socket: WebSocket | null = null;
  /** Resolves once the current socket is open and handshaken. */
  #connecting: Promise<void> | null = null;
  #nextId = 1;
  readonly #pending = new Map<number, PendingRequest>();
  readonly #scriptHashHandlers = new Map<string, Set<ScriptHashHandler>>();
  #pingTimer: ReturnType<typeof setInterval> | null = null;
  #reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  #idleTimer: ReturnType<typeof setTimeout> | null = null;
  #reconnectAttempt = 0;
  #closed = false;

  constructor(url: string, opts?: { onLog?: ElectrumLogHook }) {
    this.#url = url;
    this.#log = opts?.onLog ?? (() => {});
  }

  /**
   * Send a JSON-RPC request and await its response. Connects lazily.
   * Rejects on server error, timeout, or socket close.
   */
  async request<T>(
    method: string,
    params: unknown[],
    timeoutMs = REQUEST_TIMEOUT_MS,
  ): Promise<T> {
    await this.#ensureConnected();
    const socket = this.#socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      throw new Error("Electrum WebSocket is not connected");
    }
    const id = this.#nextId++;
    const payload = JSON.stringify({ jsonrpc: "2.0", id, method, params });
    if (this.#idleTimer) {
      clearTimeout(this.#idleTimer);
      this.#idleTimer = null;
    }
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        this.#maybeScheduleIdleClose();
        reject(
          new Error(
            `Electrum request ${method} timed out after ${timeoutMs}ms`,
          ),
        );
      }, timeoutMs);
      this.#pending.set(id, {
        resolve: (result) => resolve(result as T),
        reject,
        timer,
      });
      socket.send(payload);
    });
  }

  /**
   * Subscribe to script hash status changes. Returns the current status
   * (null = never used). `onChange` fires on every subsequent change and,
   * after a reconnect, once with the then-current status.
   */
  async subscribeScriptHash(
    scripthash: string,
    onChange: ScriptHashHandler,
  ): Promise<string | null> {
    let handlers = this.#scriptHashHandlers.get(scripthash);
    if (!handlers) {
      handlers = new Set();
      this.#scriptHashHandlers.set(scripthash, handlers);
    }
    handlers.add(onChange);
    try {
      return await this.request<string | null>(
        "blockchain.scripthash.subscribe",
        [scripthash],
      );
    } catch (error) {
      // A server-rejected subscription (or a closed client) never recovers,
      // so drop the handler. A transport failure keeps it registered: the
      // reconnect loop re-issues every registered subscription once the
      // server is reachable again, so the push self-heals.
      const permanent =
        this.#closed ||
        (error instanceof Error && error.message.startsWith("Electrum error:"));
      if (permanent) {
        handlers.delete(onChange);
        if (handlers.size === 0) this.#scriptHashHandlers.delete(scripthash);
        this.#maybeScheduleIdleClose();
      } else {
        this.#scheduleReconnect();
      }
      throw error;
    }
  }

  /** Remove a handler; unsubscribes on the wire when none remain. */
  unsubscribeScriptHash(scripthash: string, onChange: ScriptHashHandler): void {
    const handlers = this.#scriptHashHandlers.get(scripthash);
    if (!handlers) return;
    handlers.delete(onChange);
    if (handlers.size > 0) return;
    this.#scriptHashHandlers.delete(scripthash);
    if (this.#socket?.readyState === WebSocket.OPEN) {
      this.request("blockchain.scripthash.unsubscribe", [scripthash]).catch(
        () => {
          // Best-effort: the server drops the subscription on disconnect anyway.
        },
      );
    }
    this.#maybeScheduleIdleClose();
  }

  /** Close the socket and reject all in-flight requests. */
  close(): void {
    this.#closed = true;
    if (this.#reconnectTimer) {
      clearTimeout(this.#reconnectTimer);
      this.#reconnectTimer = null;
    }
    this.#teardownSocket(new Error("Electrum client closed"));
    this.#scriptHashHandlers.clear();
  }

  async #ensureConnected(): Promise<void> {
    if (this.#closed) throw new Error("Electrum client closed");
    if (this.#socket?.readyState === WebSocket.OPEN) return;
    this.#connecting ??= this.#connect().finally(() => {
      this.#connecting = null;
    });
    return this.#connecting;
  }

  #connect(): Promise<void> {
    if (typeof WebSocket === "undefined") {
      return Promise.reject(
        new Error("WebSocket is not available in this runtime"),
      );
    }
    return new Promise<void>((resolve, reject) => {
      const socket = new WebSocket(this.#url);
      this.#socket = socket;
      const connectTimer = setTimeout(() => {
        reject(
          new Error(
            `Electrum WebSocket connect to ${this.#url} timed out after ${CONNECT_TIMEOUT_MS}ms`,
          ),
        );
        try {
          socket.close();
        } catch {
          // ignore
        }
      }, CONNECT_TIMEOUT_MS);

      socket.addEventListener("open", () => {
        clearTimeout(connectTimer);
        this.#reconnectAttempt = 0;
        this.#startPing();
        this.#log("debug", "electrum.connected", "Electrum WebSocket open", {
          url: this.#url,
        });
        // Handshake (some servers require it before other calls), then
        // re-issue any active subscriptions from before a reconnect.
        this.request("server.version", ["lendaswap-sdk", "1.4"]).catch(() => {
          // Non-fatal: Fulcrum answers other methods regardless.
        });
        for (const scripthash of this.#scriptHashHandlers.keys()) {
          this.request<string | null>("blockchain.scripthash.subscribe", [
            scripthash,
          ])
            .then((status) => this.#dispatchScriptHash(scripthash, status))
            .catch(() => {
              // The next reconnect (or caller-level safety re-check) covers this.
            });
        }
        resolve();
      });

      socket.addEventListener("message", (evt) => {
        this.#onMessage(evt.data);
      });

      socket.addEventListener("error", () => {
        clearTimeout(connectTimer);
        reject(
          new Error(`Electrum WebSocket error connecting to ${this.#url}`),
        );
      });

      socket.addEventListener("close", () => {
        clearTimeout(connectTimer);
        reject(new Error("Electrum WebSocket closed during connect"));
        this.#teardownSocket(new Error("Electrum WebSocket closed"));
        if (!this.#closed && this.#scriptHashHandlers.size > 0) {
          this.#scheduleReconnect();
        }
      });
    });
  }

  #teardownSocket(error: Error): void {
    this.#stopPing();
    if (this.#idleTimer) {
      clearTimeout(this.#idleTimer);
      this.#idleTimer = null;
    }
    const socket = this.#socket;
    this.#socket = null;
    if (socket && socket.readyState !== WebSocket.CLOSED) {
      try {
        socket.close();
      } catch {
        // ignore
      }
    }
    for (const [id, pending] of this.#pending) {
      clearTimeout(pending.timer);
      pending.reject(error);
      this.#pending.delete(id);
    }
  }

  #scheduleReconnect(): void {
    if (this.#reconnectTimer) return;
    const delay = Math.min(
      MAX_RECONNECT_MS,
      MIN_RECONNECT_MS * 2 ** this.#reconnectAttempt,
    );
    this.#reconnectAttempt += 1;
    this.#reconnectTimer = setTimeout(() => {
      this.#reconnectTimer = null;
      if (this.#closed || this.#scriptHashHandlers.size === 0) return;
      this.#ensureConnected().catch(() => this.#scheduleReconnect());
    }, delay);
  }

  /**
   * Close an idle socket after {@link IDLE_CLOSE_MS}. Subscriptions and
   * in-flight requests keep it open; the keepalive ping only runs while a
   * socket exists, so a closed idle socket costs nothing.
   */
  #maybeScheduleIdleClose(): void {
    if (this.#idleTimer || this.#closed) return;
    if (this.#pending.size > 0 || this.#scriptHashHandlers.size > 0) return;
    if (!this.#socket) return;
    this.#idleTimer = setTimeout(() => {
      this.#idleTimer = null;
      if (this.#pending.size > 0 || this.#scriptHashHandlers.size > 0) return;
      this.#teardownSocket(new Error("Electrum connection idle-closed"));
    }, IDLE_CLOSE_MS);
  }

  #startPing(): void {
    this.#stopPing();
    this.#pingTimer = setInterval(() => {
      if (this.#socket?.readyState !== WebSocket.OPEN) return;
      this.request("server.ping", []).catch(() => {
        // A half-open socket (NAT/proxy blackhole after a network change) can
        // stay OPEN yet never answer — no close event ever fires. Tear it
        // down ourselves so the reconnect path runs instead of every later
        // request timing out against a dead socket.
        if (this.#socket && this.#socket.readyState === WebSocket.OPEN) {
          this.#teardownSocket(new Error("Electrum keepalive timed out"));
          if (!this.#closed && this.#scriptHashHandlers.size > 0) {
            this.#scheduleReconnect();
          }
        }
      });
    }, PING_INTERVAL_MS);
  }

  #stopPing(): void {
    if (this.#pingTimer) {
      clearInterval(this.#pingTimer);
      this.#pingTimer = null;
    }
  }

  #onMessage(data: unknown): void {
    if (typeof data !== "string") return;
    // Fulcrum newline-terminates frames; be tolerant of batched lines.
    for (const line of data.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      let parsed: {
        id?: number;
        result?: unknown;
        error?: { code?: number; message?: string } | null;
        method?: string;
        params?: unknown[];
      };
      try {
        parsed = JSON.parse(trimmed);
      } catch {
        continue;
      }
      if (parsed.id !== undefined && parsed.id !== null) {
        const pending = this.#pending.get(parsed.id);
        if (!pending) continue;
        this.#pending.delete(parsed.id);
        clearTimeout(pending.timer);
        this.#maybeScheduleIdleClose();
        if (parsed.error) {
          pending.reject(
            new Error(
              `Electrum error: ${parsed.error.message ?? JSON.stringify(parsed.error)}`,
            ),
          );
        } else {
          pending.resolve(parsed.result);
        }
      } else if (parsed.method === "blockchain.scripthash.subscribe") {
        const [scripthash, status] = (parsed.params ?? []) as [
          string,
          string | null,
        ];
        this.#dispatchScriptHash(scripthash, status ?? null);
      }
    }
  }

  #dispatchScriptHash(scripthash: string, status: string | null): void {
    const handlers = this.#scriptHashHandlers.get(scripthash);
    if (!handlers) return;
    for (const handler of handlers) {
      try {
        handler(status);
      } catch (error) {
        this.#log("warn", "electrum.handlerError", "scripthash handler threw", {
          error: error instanceof Error ? error.message : String(error),
        });
      }
    }
  }
}

/**
 * Finds the largest unspent output at `address` via
 * `blockchain.scripthash.listunspent` (includes mempool entries).
 * Mirrors the shape of `esplora.findOutputByAddress` — largest rather than
 * first, because the HTLC address is public and a stray dust output must
 * not shadow the real deposit (a sub-fee UTXO would make every claim or
 * refund built against it fail).
 */
export async function electrumFindOutputByAddress(
  client: ElectrumWsClient,
  address: string,
  network: ElectrumNetwork,
): Promise<{ txid: string; vout: number; amount: bigint } | null> {
  const scripthash = addressToScriptHash(address, network);
  const utxos = await client.request<ElectrumUnspent[]>(
    "blockchain.scripthash.listunspent",
    [scripthash],
  );
  if (!utxos || utxos.length === 0) return null;
  const utxo = utxos.reduce((best, candidate) =>
    candidate.value > best.value ? candidate : best,
  );
  return {
    txid: utxo.tx_hash,
    vout: utxo.tx_pos,
    amount: BigInt(utxo.value),
  };
}

/**
 * Fetches a transaction's outputs via `blockchain.transaction.get` (raw hex,
 * parsed locally). Returns null when the server doesn't know the tx yet, so
 * callers can fall back or keep waiting — mirrors
 * `esplora.fetchTransactionOutputs`.
 */
export async function electrumFetchTransactionOutputs(
  client: ElectrumWsClient,
  txid: string,
): Promise<{ vout: Array<{ value: number }> } | null> {
  let rawHex: string;
  try {
    rawHex = await client.request<string>("blockchain.transaction.get", [txid]);
  } catch {
    return null;
  }
  try {
    const tx = btc.Transaction.fromRaw(hexToBytes(rawHex), {
      allowUnknownOutputs: true,
      allowUnknownInputs: true,
      disableScriptCheck: true,
    });
    const vout: Array<{ value: number }> = [];
    for (let i = 0; i < tx.outputsLength; i++) {
      vout.push({ value: Number(tx.getOutput(i).amount ?? 0n) });
    }
    return { vout };
  } catch {
    return null;
  }
}

/**
 * Broadcasts a raw transaction via `blockchain.transaction.broadcast`.
 * Returns the txid; throws with the server's rejection message (kept
 * verbatim so `bad-txns-*` style errors remain classifiable as permanent).
 */
export async function electrumBroadcastTransaction(
  client: ElectrumWsClient,
  txHex: string,
): Promise<string> {
  return client.request<string>(
    "blockchain.transaction.broadcast",
    [txHex],
    BROADCAST_TIMEOUT_MS,
  );
}

/**
 * Waits for an unspent output at `address`, driven by scripthash push
 * notifications instead of polling. Resolves null on timeout. A slow
 * safety re-check covers a missed notification; the initial check covers
 * an already-funded address.
 */
export async function electrumWaitForOutputByAddress(
  client: ElectrumWsClient,
  address: string,
  network: ElectrumNetwork,
  timeoutMs: number,
  opts?: { safetyRecheckMs?: number },
): Promise<{ txid: string; vout: number; amount: bigint } | null> {
  const scripthash = addressToScriptHash(address, network);
  const safetyRecheckMs = opts?.safetyRecheckMs ?? 5_000;

  return new Promise((resolve, reject) => {
    let settled = false;
    let deadlineTimer: ReturnType<typeof setTimeout> | null = null;
    let safetyTimer: ReturnType<typeof setInterval> | null = null;
    /** Serializes checks so a notification burst doesn't stack lookups. */
    let checking = false;

    const finish = (
      outcome:
        | { ok: { txid: string; vout: number; amount: bigint } | null }
        | { err: Error },
    ) => {
      if (settled) return;
      settled = true;
      if (deadlineTimer) clearTimeout(deadlineTimer);
      if (safetyTimer) clearInterval(safetyTimer);
      client.unsubscribeScriptHash(scripthash, onChange);
      if ("err" in outcome) reject(outcome.err);
      else resolve(outcome.ok);
    };

    const check = async () => {
      if (settled || checking) return;
      checking = true;
      try {
        const output = await electrumFindOutputByAddress(
          client,
          address,
          network,
        );
        if (output) finish({ ok: output });
      } catch {
        // Transient lookup failure; the safety timer retries.
      } finally {
        checking = false;
      }
    };

    const onChange = () => {
      void check();
    };

    deadlineTimer = setTimeout(
      () => finish({ ok: null }),
      Math.max(0, timeoutMs),
    );
    safetyTimer = setInterval(() => void check(), safetyRecheckMs);

    client.subscribeScriptHash(scripthash, onChange).then(
      (status) => {
        // Non-null status means the script has history — check immediately.
        // Check on null too: cheap, and guards against a stale status.
        void status;
        void check();
      },
      (error) =>
        finish({
          err: error instanceof Error ? error : new Error(String(error)),
        }),
    );
  });
}
