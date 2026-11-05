import { hexToBytes } from "@noble/hashes/utils.js";
import { bech32 } from "@scure/base";

// BOLT 11 spec example: "Please send $3 for a cup of coffee to the same
// peer, within one minute"; 2500u = 250000 sats, payment hash 0001…0102.
export const INVOICE =
  "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53ut353s06fv3qfegext0eh0ymjpf39tuven09sam30g4vgpfna3rh";
export const PAYMENT_HASH =
  "0x0001020304050607080900010203040506070809000102030405060708090102";
export const INVOICE_SATS = 250000;

/** {@link INVOICE} with another payment hash (its signature is not checked). */
export function invoiceWithHash(hash: string): string {
  const { prefix, words } = bech32.decode(
    INVOICE as `${string}1${string}`,
    false,
  );
  const hashWords = bech32.toWords(hexToBytes(hash.slice(2)));
  // Walk the tagged fields `type(1) length(2) data(length)` after the
  // 7-word timestamp to the payment hash (tag 1).
  let i = 7;
  while (words[i] !== 1) i += 3 + ((words[i + 1] << 5) | words[i + 2]);
  return bech32.encode(
    prefix,
    [
      ...words.slice(0, i + 3),
      ...hashWords,
      ...words.slice(i + 3 + hashWords.length),
    ],
    false,
  );
}
