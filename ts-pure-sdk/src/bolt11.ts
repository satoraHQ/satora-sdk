/**
 * The little of BOLT 11 the SDK needs: an invoice's payment hash, read from
 * the `p` tagged field. Amount, expiry and routing stay the server's job.
 */

import { bytesToHex } from "@noble/hashes/utils.js";
import { bech32 } from "@scure/base";

const TAG_PAYMENT_HASH = 1;
const TIMESTAMP_WORDS = 7;
const PAYMENT_HASH_WORDS = 52;

/**
 * The payment hash of a BOLT 11 invoice as 0x-prefixed lowercase hex.
 *
 * Accepts a `lightning:` prefix and mixed case. Throws when the string is
 * not a bech32 Lightning invoice or carries no 32-byte payment hash.
 */
export function invoicePaymentHash(invoice: string): string {
  const value = invoice
    .trim()
    .replace(/^lightning:/i, "")
    .toLowerCase();
  if (!/^ln[a-z0-9]+1[02-9ac-hj-np-z]+$/.test(value)) {
    throw new Error("Not a Lightning invoice");
  }
  const { words } = bech32.decode(value as `${string}1${string}`, false);
  // Timestamp, then tagged fields `type(1) length(2) data(length)`, then
  // the 104-word signature.
  let i = TIMESTAMP_WORDS;
  const end = words.length - 104;
  while (i + 3 <= end) {
    const tag = words[i];
    const length = (words[i + 1] << 5) | words[i + 2];
    const data = words.slice(i + 3, i + 3 + length);
    if (tag === TAG_PAYMENT_HASH && length === PAYMENT_HASH_WORDS) {
      return `0x${bytesToHex(bech32.fromWords(data))}`;
    }
    i += 3 + length;
  }
  throw new Error("Lightning invoice carries no payment hash");
}

/**
 * The amount a BOLT 11 invoice asks for, in millisatoshis, read from its
 * human-readable part; undefined for an invoice without an amount.
 */
export function invoiceAmountMsat(invoice: string): bigint | undefined {
  const value = invoice
    .trim()
    .replace(/^lightning:/i, "")
    .toLowerCase();
  const separator = value.lastIndexOf("1");
  const match = /^ln(?:bcrt|tbs|bc|tb|sb)(\d*)([munp]?)$/.exec(
    value.slice(0, separator),
  );
  if (!match) throw new Error("Not a Lightning invoice");
  const [, digits, multiplier] = match;
  if (digits === "") return undefined;
  // 1 BTC = 10^11 msat; each multiplier divides it further.
  const amount = BigInt(digits);
  switch (multiplier) {
    case "":
      return amount * 100_000_000_000n;
    case "m":
      return amount * 100_000_000n;
    case "u":
      return amount * 100_000n;
    case "n":
      return amount * 100n;
    case "p":
      if (amount % 10n !== 0n) {
        throw new Error("Invoice amount is not a whole millisatoshi");
      }
      return amount / 10n;
  }
  throw new Error("Not a Lightning invoice");
}
