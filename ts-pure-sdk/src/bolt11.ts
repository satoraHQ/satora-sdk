/**
 * The little of BOLT 11 the SDK needs: an invoice's payment hash, read from
 * the `p` tagged field. Amount, expiry and routing stay the server's job.
 *
 * The signature is not checked: the party being checked is the server, which
 * signs whatever invoice it issues, and the paying wallet verifies the
 * signature itself. What matters is the hash that wallet commits to.
 */

import { bech32 } from "@scure/base";

const TAG_PAYMENT_HASH = 1;
const TIMESTAMP_WORDS = 7;
const SIGNATURE_WORDS = 104;
const PAYMENT_HASH_WORDS = 52;

/**
 * The payment hash of a BOLT 11 invoice as 0x-prefixed lowercase hex.
 *
 * Read the way LDK reads it, which is what the swap server parses with: a `p`
 * field of any length other than 52 words is skipped, a second 52-word `p`
 * field is refused (LND would pay the first, LDK refuses the invoice, so the
 * two could disagree on the hash), and the 52 words give 260 bits of which
 * the first 256 are the hash.
 *
 * Accepts a `lightning:` prefix and mixed case. Throws when the string is
 * not a bech32 Lightning invoice, a tagged field runs past the signature, or
 * there is not exactly one payment hash.
 */
export function invoicePaymentHash(invoice: string): string {
  const value = invoice
    .trim()
    .replace(/^lightning:/i, "")
    .toLowerCase();
  if (!/^ln[a-z0-9]+1[02-9ac-hj-np-z]+$/.test(value)) {
    throw new Error("Not a Lightning invoice");
  }
  let words: number[];
  try {
    ({ words } = bech32.decode(value as `${string}1${string}`, false));
  } catch (e) {
    throw new Error(`Not a Lightning invoice: ${(e as Error).message}`);
  }
  if (words.length < TIMESTAMP_WORDS + SIGNATURE_WORDS) {
    throw new Error("Lightning invoice is too short");
  }
  // Timestamp, then tagged fields `type(1) length(2) data(length)`, then
  // the signature.
  const fields = words.slice(TIMESTAMP_WORDS, words.length - SIGNATURE_WORDS);
  let paymentHash: string | undefined;
  let i = 0;
  while (i < fields.length) {
    if (i + 3 > fields.length) {
      throw new Error("Lightning invoice has a truncated tagged field");
    }
    const tag = fields[i];
    const length = (fields[i + 1] << 5) | fields[i + 2];
    const start = i + 3;
    if (start + length > fields.length) {
      throw new Error("Lightning invoice has a truncated tagged field");
    }
    if (tag === TAG_PAYMENT_HASH && length === PAYMENT_HASH_WORDS) {
      if (paymentHash !== undefined) {
        throw new Error("Lightning invoice carries more than one payment hash");
      }
      paymentHash = first32Bytes(fields.slice(start, start + length));
    }
    i = start + length;
  }
  if (paymentHash === undefined) {
    throw new Error("Lightning invoice carries no payment hash");
  }
  return `0x${paymentHash}`;
}

/** The first 32 bytes packed from 5-bit words, as hex; the rest is dropped. */
function first32Bytes(words: number[]): string {
  const bytes: number[] = [];
  let acc = 0;
  let bits = 0;
  for (const word of words) {
    acc = (acc << 5) | word;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      if (bytes.length < 32) bytes.push((acc >> bits) & 0xff);
    }
    acc &= (1 << bits) - 1;
  }
  return bytes.map((b) => b.toString(16).padStart(2, "0")).join("");
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
