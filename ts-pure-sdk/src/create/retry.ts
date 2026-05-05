/**
 * Retry helper for swap creation when the server rejects a duplicate hash lock,
 * and error types for non-retryable duplicate conditions.
 */

import { createSdkLogger } from "../logging.js";
import type { CreateSwapContext } from "./types.js";

const MAX_RETRIES = 10;
const SKIP_COUNT = 10;

/**
 * Thrown when the server rejects a submarine swap because the Lightning
 * invoice has already been used in another swap. This cannot be retried
 * automatically — the caller must provide a different invoice.
 */
export class DuplicateInvoiceError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "DuplicateInvoiceError";
  }
}

/**
 * Returns true if the error indicates a hash lock collision (409 Conflict
 * from the server, or a rejection for a reused preimage hash).
 */
function isHashCollisionError(message: string): boolean {
  return message.includes("a swap with this preimage hash exists already");
}

/**
 * Returns true if the error indicates a duplicate Lightning invoice.
 */
export function isDuplicateInvoiceError(message: string): boolean {
  return message.includes("a swap with this invoice exists already");
}

/**
 * Wraps a swap creation attempt with automatic retry on hash lock collisions.
 *
 * When the server returns 409 (duplicate hash lock) or rejects the
 * preimage, this skips the key index forward and retries with fresh params.
 *
 * @param ctx - The swap creation context (must include skipKeyIndices).
 * @param attempt - A function that derives params and creates the swap.
 *                  Called once per attempt; must call ctx.deriveSwapParams() internally.
 */
export async function retryOnHashCollision<T>(
  ctx: CreateSwapContext,
  attempt: () => Promise<T>,
): Promise<T> {
  let lastError: Error | undefined;
  const logger = createSdkLogger(ctx).child({
    module: "create/retry",
    operation: "create.retry_on_hash_collision",
  });

  for (let i = 0; i < MAX_RETRIES; i++) {
    try {
      return await attempt();
    } catch (e) {
      logger.debug({
        event: "create.retry.attempt_failed",
        message: "Swap creation attempt failed",
        data: { attempt: i + 1 },
        error: e,
      });

      if (e instanceof Error && isHashCollisionError(e.message)) {
        lastError = e;
        if (ctx.skipKeyIndices) {
          await ctx.skipKeyIndices(SKIP_COUNT);
        }
        continue;
      }
      throw e;
    }
  }

  throw lastError;
}
