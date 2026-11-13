import { readFileSync } from "node:fs";
import { Transaction } from "@arkade-os/sdk";
import { base64, hex } from "@scure/base";
import { describe, expect, it } from "vitest";
import {
  getArkTxid,
  signEscrowArkTx,
  signEscrowCheckpoints,
  signEscrowDelegate,
} from "../src/escrow/index.js";

interface Fixture {
  buyerSecretKey: string;
  buyerPubkey: string;
  arkTx: string;
  checkpoints: string[];
  intentProof: string;
  forfeits: string[];
}

const fixture: Fixture = JSON.parse(
  readFileSync(
    new URL("./fixtures/escrow-gem-psbts.json", import.meta.url),
    "utf8",
  ),
);

const buyerSk = fixture.buyerSecretKey;

/** Parse with every check on, as any strict PSBT consumer would. */
function strict(psbtB64: string): Transaction {
  return Transaction.fromPSBT(base64.decode(psbtB64));
}

/** The x-only keys that signed input `index` through the script path. */
function signers(psbtB64: string, index: number): string[] {
  return (strict(psbtB64).getInput(index).tapScriptSig ?? []).map(
    ([{ pubKey }]) => hex.encode(pubKey),
  );
}

describe("escrow signing of PSBTs built by the ark-escrow gem", () => {
  it("the gem's PSBTs carry witnessScript on taproot inputs, which a strict parse rejects", () => {
    for (const psbt of [
      fixture.arkTx,
      fixture.checkpoints[0],
      fixture.intentProof,
      fixture.forfeits[0],
    ]) {
      expect(() => strict(psbt)).toThrow("witnessScript without P2WSH");
    }
  });

  it("signs the ark tx and returns a PSBT that parses strictly", () => {
    const { signedPsbt, txid } = signEscrowArkTx(fixture.arkTx, buyerSk);

    expect(txid).toBe(getArkTxid(fixture.arkTx));
    expect(signers(signedPsbt, 0)).toContain(fixture.buyerPubkey);
    expect(strict(signedPsbt).getInput(0).witnessScript).toBeUndefined();
  });

  it("keeps what the arbiter needs: unsigned tx, leaf script and prevout", () => {
    const lenient = (b64: string) =>
      Transaction.fromPSBT(base64.decode(b64), { disableScriptCheck: true });
    const before = lenient(fixture.arkTx);
    const after = strict(signEscrowArkTx(fixture.arkTx, buyerSk).signedPsbt);

    expect(after.id).toBe(before.id);
    expect(after.getInput(0).tapLeafScript).toEqual(
      before.getInput(0).tapLeafScript,
    );
    expect(after.getInput(0).witnessUtxo).toEqual(
      before.getInput(0).witnessUtxo,
    );
  });

  it("signs the checkpoints", () => {
    const signed = signEscrowCheckpoints(fixture.checkpoints, buyerSk);

    expect(signed).toHaveLength(fixture.checkpoints.length);
    for (const checkpoint of signed) {
      expect(signers(checkpoint, 0)).toContain(fixture.buyerPubkey);
    }
  });

  it("signs the refresh intent proof and forfeits", async () => {
    const { signedIntentProof, signedForfeitPsbts } = await signEscrowDelegate(
      fixture.intentProof,
      fixture.forfeits,
      buyerSk,
    );

    expect(signers(signedIntentProof, 1)).toContain(fixture.buyerPubkey);
    expect(signedForfeitPsbts).toHaveLength(fixture.forfeits.length);
    for (const forfeit of signedForfeitPsbts) {
      expect(signers(forfeit, 0)).toContain(fixture.buyerPubkey);
    }
  });

  it("signs a PSBT that never had the stray field", () => {
    const tx = Transaction.fromPSBT(base64.decode(fixture.arkTx), {
      disableScriptCheck: true,
    });
    tx.updateInput(0, { witnessScript: undefined }, true);
    const clean = base64.encode(tx.toPSBT());
    expect(() => strict(clean)).not.toThrow();

    const { signedPsbt, txid } = signEscrowArkTx(clean, buyerSk);

    expect(txid).toBe(getArkTxid(fixture.arkTx));
    expect(signers(signedPsbt, 0)).toContain(fixture.buyerPubkey);
  });
});
