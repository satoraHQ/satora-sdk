import { bech32 } from "@scure/base";
import { describe, expect, it } from "vitest";
import { invoicePaymentHash } from "../src/bolt11.js";
import vectors from "./fixtures/lightning-invoices.js";

const LND = vectors.invoices.lnd_hold_index_0;

/**
 * Re-encodes a real invoice with its tagged fields edited. The checksum is
 * valid and the signature is not, which the decoder does not look at.
 */
function withFields(
  invoice: string,
  edit: (fields: number[][]) => number[][],
): string {
  const { prefix, words } = bech32.decode(
    invoice as `${string}1${string}`,
    false,
  );
  const timestamp = words.slice(0, 7);
  const signature = words.slice(-104);
  const tagged = words.slice(7, -104);
  const fields: number[][] = [];
  for (let i = 0; i < tagged.length; ) {
    const length = tagged[i + 1] * 32 + tagged[i + 2];
    fields.push(tagged.slice(i, i + 3 + length));
    i += 3 + length;
  }
  return bech32.encode(
    prefix,
    [...timestamp, ...edit(fields).flat(), ...signature],
    false,
  );
}

function field(tag: number, data: number[]): number[] {
  return [tag, data.length >> 5, data.length & 31, ...data];
}

const isPaymentHash = (f: number[]) => f[0] === 1 && f[1] * 32 + f[2] === 52;

describe("invoicePaymentHash", () => {
  const realInvoices = [
    ...Object.values(vectors.invoices),
    {
      bolt11: vectors.swaps.lightning_to_arkade.response.bolt11_invoice,
      payment_hash: vectors.swaps.lightning_to_arkade.invoice_payment_hash,
    },
    {
      bolt11:
        vectors.swaps.arkade_to_lightning.response.client_lightning_invoice,
      payment_hash: vectors.swaps.arkade_to_lightning.invoice_payment_hash,
    },
  ];

  it.each(realInvoices)("reads $payment_hash", ({ bolt11, payment_hash }) => {
    expect(invoicePaymentHash(bolt11)).toBe(`0x${payment_hash}`);
  });

  it("reads an uppercase invoice, as a QR code carries it", () => {
    expect(invoicePaymentHash(LND.bolt11.toUpperCase())).toBe(
      `0x${LND.payment_hash}`,
    );
  });

  it("refuses an invoice with a second payment hash", () => {
    const twice = withFields(LND.bolt11, (fields) => [
      ...fields,
      field(1, Array(52).fill(4)),
    ]);
    expect(() => invoicePaymentHash(twice)).toThrow(
      /more than one payment hash/,
    );
  });

  it("refuses an invoice without a payment hash", () => {
    const none = withFields(LND.bolt11, (fields) =>
      fields.filter((f) => !isPaymentHash(f)),
    );
    expect(() => invoicePaymentHash(none)).toThrow(/no payment hash/);
  });

  it("skips a p field that is not 52 words long, as LDK does", () => {
    const short = field(1, Array(51).fill(4));
    const before = withFields(LND.bolt11, (fields) => [short, ...fields]);
    expect(invoicePaymentHash(before)).toBe(`0x${LND.payment_hash}`);

    const only = withFields(LND.bolt11, (fields) =>
      fields.map((f) => (isPaymentHash(f) ? short : f)),
    );
    expect(() => invoicePaymentHash(only)).toThrow(/no payment hash/);
  });

  it("takes the first 256 bits of the field, dropping set padding bits", () => {
    const padded = withFields(LND.bolt11, (fields) =>
      fields.map((f) => {
        if (!isPaymentHash(f)) return f;
        const edited = [...f];
        edited[edited.length - 1] |= 0b01111;
        return edited;
      }),
    );
    expect(invoicePaymentHash(padded)).toBe(`0x${LND.payment_hash}`);
  });

  it("refuses a tagged field that runs into the signature", () => {
    const truncated = withFields(LND.bolt11, (fields) => [
      ...fields,
      [13, 1, 0, 5, 5],
    ]);
    expect(() => invoicePaymentHash(truncated)).toThrow(/truncated/);
  });

  it("refuses a bad checksum and what is not a Lightning invoice", () => {
    const flipped = `${LND.bolt11.slice(0, -1)}${LND.bolt11.endsWith("q") ? "p" : "q"}`;
    expect(() => invoicePaymentHash(flipped)).toThrow(
      /Not a Lightning invoice/,
    );
    expect(() =>
      invoicePaymentHash("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4"),
    ).toThrow(/Not a Lightning invoice/);
    expect(() => invoicePaymentHash("lnbcrt1fixture")).toThrow(
      /Not a Lightning invoice/,
    );
    const { words } = bech32.decode(LND.bolt11 as `${string}1${string}`, false);
    expect(() =>
      invoicePaymentHash(bech32.encode("bcrt", words, false)),
    ).toThrow(/Not a Lightning invoice/);
  });
});
