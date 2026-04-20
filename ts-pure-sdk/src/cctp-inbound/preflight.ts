/**
 * Optional per-call pre-flight for the CCTP-inbound UserOp batch.
 *
 * The bundler simulates the whole UserOp and returns a single
 * "execution reverted" if any inner call fails — useless for
 * iterating. This helper runs each call individually via `eth_call`
 * with the smart account as `from`, surfacing the per-call revert
 * data before we even talk to the bundler.
 *
 * Limitation: any call that re-enters the smart account (e.g. Permit2
 * invoking `IERC1271.isValidSignature`) reverts here because the
 * account isn't deployed at simulation time but WILL be in the real
 * UserOp via factoryData. Treat these reverts as informational — the
 * bundler's full-simulation is the authoritative check.
 */

import type { Address, PublicClient } from "viem";
import type { BatchCall } from "./userOp.js";

/**
 * Walk a viem error chain and surface any RPC-level revert data
 * (selector + encoded args). viem buries these under `cause` /
 * `data` / nested `data.data`.
 */
export function extractRevertData(err: unknown): string | undefined {
  let cur: unknown = err;
  for (let depth = 0; depth < 10 && cur; depth++) {
    const node = cur as { data?: unknown; cause?: unknown };
    if (typeof node.data === "string" && node.data.startsWith("0x")) {
      return node.data;
    }
    if (typeof node.data === "object" && node.data !== null) {
      const nested = (node.data as { data?: string }).data;
      if (typeof nested === "string" && nested.startsWith("0x")) {
        return nested;
      }
    }
    cur = node.cause;
  }
  return undefined;
}

export interface SimulateBatchCallsArgs {
  calls: BatchCall[];
  smartAccount: Address;
  publicClient: PublicClient;
}

/**
 * Run `eth_call` once per batch call with the smart account as
 * `from`. Logs per-call status via `console.log` / `console.warn` so
 * failures are visible in devtools. Never throws — pre-flight reverts
 * for calls that re-enter the smart account are expected.
 */
export async function simulateBatchCalls(
  args: SimulateBatchCallsArgs,
): Promise<void> {
  const { calls, smartAccount, publicClient } = args;
  for (let i = 0; i < calls.length; i++) {
    const call = calls[i];
    const label = `[cctp-inbound/preflight] call ${i + 1}/${calls.length} to ${call.to}`;
    try {
      await publicClient.call({
        account: smartAccount,
        to: call.to,
        data: call.data,
        value: call.value,
      });
      console.log(`${label} OK`);
    } catch (err) {
      const revertData = extractRevertData(err);
      console.warn(`${label} reverted in pre-flight (may be OK at send)`, {
        revertData: revertData ?? "(none)",
        error: err,
      });
    }
  }
}
