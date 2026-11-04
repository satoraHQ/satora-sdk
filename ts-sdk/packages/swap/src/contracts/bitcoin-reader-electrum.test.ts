import { sha256 } from "@noble/hashes/sha2.js";
import { hex } from "@scure/base";
import * as btc from "@scure/btc-signer";
import { describe, expect, it, vi } from "vitest";
import type { BitcoinChainReader } from "./bitcoin-manager.js";
import { type ElectrumRpc, electrumReader } from "./bitcoin-reader-electrum.js";

/** BIP173 test-vector P2WPKH — any address works, the reader only compares scripts. */
const HTLC_ADDRESS = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
const HTLC_OUT = btc.Address(btc.NETWORK).decode(HTLC_ADDRESS);
if (!HTLC_OUT) throw new Error("test address must decode");
const HTLC_SCRIPT = btc.OutScript.encode(HTLC_OUT);
/** A different output script, for change/unrelated outputs. */
const OTHER_SCRIPT = hex.decode("0014000000000000000000000000000000000000ffff");

type TxIn = { txid: string; vout: number; witness?: string[] };
type TxOut = { script: Uint8Array; amount: bigint };
type BuiltTx = { raw: string; txid: string };

/**
 * Hand-serialize a transaction (consensus encoding, segwit when any input has
 * a witness). Keeps the fixtures independent of btc-signer's PSBT builder and
 * exercises the reader's parsing of real wire bytes — including the txid
 * byte-order convention (double-sha256 of the witness-stripped tx, reversed).
 */
function buildTx(ins: TxIn[], outs: TxOut[]): BuiltTx {
  const u32 = (n: number) => [
    n & 0xff,
    (n >>> 8) & 0xff,
    (n >>> 16) & 0xff,
    (n >>> 24) & 0xff,
  ];
  const u64 = (n: bigint) =>
    Array.from({ length: 8 }, (_, i) => Number((n >> BigInt(8 * i)) & 0xffn));
  const body = (withWitness: boolean): Uint8Array => {
    const parts: number[] = [...u32(2)]; // version
    if (withWitness) parts.push(0x00, 0x01); // marker + flag
    parts.push(ins.length);
    for (const input of ins) {
      parts.push(...hex.decode(input.txid).reverse(), ...u32(input.vout));
      parts.push(0); // empty scriptSig
      parts.push(...u32(0xffffffff)); // sequence
    }
    parts.push(outs.length);
    for (const out of outs) {
      parts.push(...u64(out.amount), out.script.length, ...out.script);
    }
    if (withWitness) {
      for (const input of ins) {
        const items = input.witness ?? [];
        parts.push(items.length);
        for (const item of items) {
          const bytes = hex.decode(item);
          parts.push(bytes.length, ...bytes);
        }
      }
    }
    parts.push(...u32(0)); // locktime
    return Uint8Array.from(parts);
  };
  const hasWitness = ins.some((input) => (input.witness ?? []).length > 0);
  return {
    raw: hex.encode(body(hasWitness)),
    txid: hex.encode(sha256(sha256(body(false))).reverse()),
  };
}

/** A funding tx paying `sats` to the HTLC script (plus change). */
function makeFundingTx(sats: bigint): BuiltTx {
  return buildTx(
    [{ txid: "11".repeat(32), vout: 0 }],
    [
      { script: HTLC_SCRIPT, amount: sats },
      { script: OTHER_SCRIPT, amount: 1_000n },
    ],
  );
}

/** A tx spending `vout` of `fundingTxid` with the given witness. */
function makeSpendTx(
  fundingTxid: string,
  vout: number,
  witness: string[],
): BuiltTx {
  return buildTx(
    [{ txid: fundingTxid, vout, witness }],
    [{ script: OTHER_SCRIPT, amount: 4_000n }],
  );
}

class FakeElectrum implements ElectrumRpc {
  history: Array<{ tx_hash: string; height: number }> = [];
  txs = new Map<string, string>();
  tipHeight = 900_000;
  fail = false;
  subscribed = new Map<string, (status: string | null) => void>();
  requests: string[] = [];

  async request<T>(method: string, params: unknown[]): Promise<T> {
    this.requests.push(method);
    if (this.fail) throw new Error("electrum down");
    switch (method) {
      case "blockchain.scripthash.get_history":
        return this.history as T;
      case "blockchain.transaction.get": {
        const raw = this.txs.get(params[0] as string);
        if (!raw) throw new Error("No such mempool or blockchain transaction");
        return raw as T;
      }
      case "blockchain.headers.subscribe":
        return { height: this.tipHeight } as T;
      default:
        throw new Error(`unexpected method ${method}`);
    }
  }

  async subscribeScriptHash(
    scripthash: string,
    onChange: (status: string | null) => void,
  ): Promise<string | null> {
    if (this.fail) throw new Error("electrum down");
    this.subscribed.set(scripthash, onChange);
    return null;
  }

  unsubscribeScriptHash(scripthash: string): void {
    this.subscribed.delete(scripthash);
  }

  addTx(tx: BuiltTx, height: number): string {
    this.txs.set(tx.txid, tx.raw);
    this.history.push({ tx_hash: tx.txid, height });
    return tx.txid;
  }
}

describe("electrumReader", () => {
  it("reports absent for an address with no history", async () => {
    const rpc = new FakeElectrum();
    const reader = electrumReader(rpc);
    expect(await reader.getHtlcFacts(HTLC_ADDRESS)).toEqual({
      funding: "absent",
      fundedSats: 0,
    });
  });

  it("reports a mempool funding as confirmed at minConfirmations 0", async () => {
    const rpc = new FakeElectrum();
    rpc.addTx(makeFundingTx(5_000n), 0);
    const reader = electrumReader(rpc);
    expect(await reader.getHtlcFacts(HTLC_ADDRESS)).toEqual({
      funding: "confirmed",
      fundedSats: 5_000,
    });
  });

  it("reports a mempool funding as mempool at minConfirmations 1", async () => {
    const rpc = new FakeElectrum();
    rpc.addTx(makeFundingTx(5_000n), 0);
    const reader = electrumReader(rpc);
    expect(await reader.getHtlcFacts(HTLC_ADDRESS, 1)).toEqual({
      funding: "mempool",
      fundedSats: 5_000,
    });
  });

  it("applies a depth policy beyond 1 via the tip height", async () => {
    const rpc = new FakeElectrum();
    rpc.addTx(makeFundingTx(5_000n), 899_999); // depth 2 at tip 900_000
    const reader = electrumReader(rpc);
    expect(await reader.getHtlcFacts(HTLC_ADDRESS, 2)).toEqual({
      funding: "confirmed",
      fundedSats: 5_000,
    });
    expect(await reader.getHtlcFacts(HTLC_ADDRESS, 3)).toEqual({
      funding: "mempool",
      fundedSats: 5_000,
    });
  });

  it("picks the tx paying the most, not the first in history", async () => {
    const rpc = new FakeElectrum();
    // Dust lands in the history BEFORE the real funding — it must not be
    // mistaken for the funding and observe the swap as underfunded.
    rpc.addTx(makeFundingTx(330n), 899_990);
    rpc.addTx(makeFundingTx(5_000n), 0);
    const reader = electrumReader(rpc);
    expect(await reader.getHtlcFacts(HTLC_ADDRESS)).toEqual({
      funding: "confirmed",
      fundedSats: 5_000,
    });
  });

  it("detects the spend of the real funding even with dust present", async () => {
    const rpc = new FakeElectrum();
    rpc.addTx(makeFundingTx(330n), 899_980); // dust, stays unspent
    const fundingTxid = rpc.addTx(makeFundingTx(5_000n), 899_990);
    const witness = ["aa".repeat(64), "07".repeat(32), "01"];
    rpc.addTx(makeSpendTx(fundingTxid, 0, witness), 0);
    const reader = electrumReader(rpc);
    expect(await reader.getHtlcFacts(HTLC_ADDRESS)).toEqual({
      funding: "confirmed",
      fundedSats: 0,
      spendWitness: witness,
    });
  });

  it("returns the spend witness once the HTLC output is spent", async () => {
    const rpc = new FakeElectrum();
    const funding = makeFundingTx(5_000n);
    const fundingTxid = rpc.addTx(funding, 899_990);
    const witness = ["aa".repeat(64), "07".repeat(32), "01"];
    rpc.addTx(makeSpendTx(fundingTxid, 0, witness), 0);
    const reader = electrumReader(rpc);
    expect(await reader.getHtlcFacts(HTLC_ADDRESS)).toEqual({
      funding: "confirmed",
      fundedSats: 0,
      spendWitness: witness,
    });
  });

  it("ignores a tx spending a different outpoint of the funding tx", async () => {
    const rpc = new FakeElectrum();
    const funding = makeFundingTx(5_000n);
    const fundingTxid = rpc.addTx(funding, 899_990);
    // Spends the change output (vout 1), not the HTLC output (vout 0).
    rpc.addTx(makeSpendTx(fundingTxid, 1, ["bb"]), 0);
    const reader = electrumReader(rpc);
    expect(await reader.getHtlcFacts(HTLC_ADDRESS)).toEqual({
      funding: "confirmed",
      fundedSats: 5_000,
    });
  });

  it("falls back to the secondary reader when electrum errors", async () => {
    const rpc = new FakeElectrum();
    rpc.fail = true;
    const fallback: BitcoinChainReader = {
      getHtlcFacts: vi.fn(async () => ({
        funding: "mempool" as const,
        fundedSats: 42,
      })),
    };
    const reader = electrumReader(rpc, { fallback });
    expect(await reader.getHtlcFacts(HTLC_ADDRESS, 1)).toEqual({
      funding: "mempool",
      fundedSats: 42,
    });
    expect(fallback.getHtlcFacts).toHaveBeenCalledWith(HTLC_ADDRESS, 1);
  });

  it("throws without a fallback when electrum errors", async () => {
    const rpc = new FakeElectrum();
    rpc.fail = true;
    const reader = electrumReader(rpc);
    await expect(reader.getHtlcFacts(HTLC_ADDRESS)).rejects.toThrow(
      /electrum down/,
    );
  });

  it("returns a noop unsubscribe when the address does not decode", async () => {
    const rpc = new FakeElectrum();
    const reader = electrumReader(rpc); // mainnet reader
    // A signet address under mainnet parameters must not throw out of
    // subscribe (it would abort the manager's register()).
    const unsubscribe = reader.subscribe?.(
      "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx",
      vi.fn(),
    );
    expect(unsubscribe).toBeTypeOf("function");
    expect(rpc.subscribed.size).toBe(0);
    unsubscribe?.();
  });

  it("subscribes and forwards history-change pushes", async () => {
    const rpc = new FakeElectrum();
    const reader = electrumReader(rpc);
    const onChange = vi.fn();
    const unsubscribe = reader.subscribe?.(HTLC_ADDRESS, onChange);
    await Promise.resolve(); // let the async subscribe land
    expect(rpc.subscribed.size).toBe(1);
    for (const handler of rpc.subscribed.values()) handler("newstatus");
    expect(onChange).toHaveBeenCalledTimes(1);
    unsubscribe?.();
    expect(rpc.subscribed.size).toBe(0);
  });
});
