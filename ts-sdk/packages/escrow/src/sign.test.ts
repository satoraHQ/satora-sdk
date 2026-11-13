import { readFileSync } from "node:fs";
import { Transaction } from "@arkade-os/sdk";
import { base64, hex } from "@scure/base";
import { describe, expect, it } from "vitest";
import { signEscrowArkTx, signEscrowCheckpoints } from "./sign.js";

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

const buyerSk = hex.decode(fixture.buyerSecretKey);

/** Parse with every check on, as any strict PSBT consumer would. */
function strict(psbtB64: string): Transaction {
  return Transaction.fromPSBT(base64.decode(psbtB64));
}

/** The x-only keys that signed input 0 through the script path. */
function signers(psbtB64: string): string[] {
  return (strict(psbtB64).getInput(0).tapScriptSig ?? []).map(([{ pubKey }]) =>
    hex.encode(pubKey),
  );
}

describe("escrow signing of PSBTs built by the ark-escrow gem", () => {
  it("the gem's PSBTs carry witnessScript on taproot inputs, which a strict parse rejects", () => {
    expect(() => strict(fixture.arkTx)).toThrow("witnessScript without P2WSH");
    expect(() => strict(fixture.checkpoints[0])).toThrow(
      "witnessScript without P2WSH",
    );
  });

  it("signs the ark tx and returns a PSBT that parses strictly", () => {
    const { signedPsbt, txid } = signEscrowArkTx(fixture.arkTx, buyerSk);

    expect(txid).toBe(strict(signedPsbt).id);
    expect(signers(signedPsbt)).toContain(fixture.buyerPubkey);
    expect(strict(signedPsbt).getInput(0).witnessScript).toBeUndefined();
  });

  it("signs the checkpoints", () => {
    const signed = signEscrowCheckpoints(fixture.checkpoints, buyerSk);

    expect(signed).toHaveLength(fixture.checkpoints.length);
    for (const checkpoint of signed) {
      expect(signers(checkpoint)).toContain(fixture.buyerPubkey);
    }
  });
});
