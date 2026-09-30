/**
 * The few Solana JSON-RPC calls the Solana CCTP burn needs, over `fetch`.
 */

import { base64 } from "@scure/base";

async function rpc<T>(
  rpcUrl: string,
  method: string,
  params: unknown[],
): Promise<T> {
  const res = await fetch(rpcUrl, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
  });
  if (!res.ok) {
    throw new Error(`Solana RPC ${method} failed: HTTP ${res.status}`);
  }
  const json = (await res.json()) as {
    result?: T;
    error?: { message: string };
  };
  if (json.error) {
    throw new Error(`Solana RPC ${method} failed: ${json.error.message}`);
  }
  return json.result as T;
}

export async function getLatestBlockhash(
  rpcUrl: string,
): Promise<{ blockhash: string; lastValidBlockHeight: number }> {
  const result = await rpc<{
    value: { blockhash: string; lastValidBlockHeight: number };
  }>(rpcUrl, "getLatestBlockhash", [{ commitment: "confirmed" }]);
  return result.value;
}

export async function getBlockHeight(rpcUrl: string): Promise<number> {
  return rpc<number>(rpcUrl, "getBlockHeight", [{ commitment: "confirmed" }]);
}

export interface SignatureStatus {
  confirmationStatus?: "processed" | "confirmed" | "finalized";
  err: unknown;
}

export async function getSignatureStatus(
  rpcUrl: string,
  signature: string,
): Promise<SignatureStatus | null> {
  const result = await rpc<{ value: (SignatureStatus | null)[] }>(
    rpcUrl,
    "getSignatureStatuses",
    [[signature], { searchTransactionHistory: true }],
  );
  return result.value[0] ?? null;
}

/**
 * Simulate an unsigned transaction (signature checks off, blockhash
 * replaced). Returns the program error and logs, or `null` on success.
 */
export async function simulateTransaction(
  rpcUrl: string,
  transactionBase64: string,
): Promise<{ err: unknown; logs: string[] } | null> {
  const result = await rpc<{
    value: { err: unknown; logs: string[] | null };
  }>(rpcUrl, "simulateTransaction", [
    transactionBase64,
    {
      encoding: "base64",
      sigVerify: false,
      replaceRecentBlockhash: true,
      commitment: "confirmed",
    },
  ]);
  if (result.value.err == null) return null;
  return { err: result.value.err, logs: result.value.logs ?? [] };
}

/**
 * A priority fee (micro-lamports per compute unit) that should land the
 * burn promptly: the 75th percentile of recent non-zero fees paid on the
 * accounts the burn writes, clamped to a sane range.
 */
export async function suggestPriorityFee(
  rpcUrl: string,
  writableAccounts: string[],
): Promise<bigint> {
  const MIN = 10_000n;
  const MAX = 2_000_000n;
  try {
    const fees = await rpc<{ prioritizationFee: number }[]>(
      rpcUrl,
      "getRecentPrioritizationFees",
      [writableAccounts],
    );
    const paid = fees
      .map((f) => f.prioritizationFee)
      .filter((f) => f > 0)
      .sort((a, b) => a - b);
    if (paid.length === 0) return MIN;
    const p75 = BigInt(paid[Math.floor(paid.length * 0.75)] ?? 0);
    return p75 < MIN ? MIN : p75 > MAX ? MAX : p75;
  } catch {
    return MIN;
  }
}

/**
 * The balance (smallest units) of an SPL token account; `0n` when the account
 * does not exist. Reads the raw account (amount at bytes 64..72) because
 * public RPCs gate the indexed token methods.
 */
export async function getTokenAccountAmount(
  rpcUrl: string,
  tokenAccount: string,
): Promise<bigint> {
  const result = await rpc<{
    value: { data: [string, string] } | null;
  }>(rpcUrl, "getAccountInfo", [
    tokenAccount,
    { encoding: "base64", commitment: "confirmed" },
  ]);
  if (!result.value) return 0n;
  const data = base64.decode(result.value.data[0]);
  if (data.length < 72) return 0n;
  return new DataView(data.buffer, data.byteOffset).getBigUint64(64, true);
}
