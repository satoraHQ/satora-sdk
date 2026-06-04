import { type ArkInfo, CSVMultisigTapscript, networks } from "@arkade-os/sdk";
import { schnorr } from "@noble/curves/secp256k1.js";
import { hex } from "@scure/base";
import { describe, expect, it } from "vitest";
import { type EscrowScriptOptions, EscrowVtxoScript } from "./escrow-script.js";
import {
  buildEscrowReleaseTx,
  type EscrowArkConfig,
  escrowArkConfigFromInfo,
  signEscrowReleaseInPlace,
} from "./release.js";
import { verifyReleaseArkTx } from "./verify.js";

function xOnlyPubKey(seed: number): Uint8Array {
  const sk = new Uint8Array(32);
  sk[31] = seed;
  return schnorr.getPublicKey(sk);
}

const sellerPubKey = xOnlyPubKey(1);
const arbiterPubKey = xOnlyPubKey(2);
const aspPubKey = xOnlyPubKey(3);
const exitTimelock = { type: "blocks", value: 4320n } as const;
const network = networks.regtest;

const options: EscrowScriptOptions = {
  sellerPubKey,
  arbiterPubKey,
  aspPubKey,
  exitTimelock,
};

/** A valid, distinct Ark address derived from a throwaway escrow script. */
function arkAddress(seed: number): string {
  return new EscrowVtxoScript({
    sellerPubKey: xOnlyPubKey(seed),
    arbiterPubKey: xOnlyPubKey(seed + 1),
    aspPubKey,
    exitTimelock,
  }).arkAddress(network);
}

const serverUnrollScript = CSVMultisigTapscript.encode({
  timelock: exitTimelock,
  pubkeys: [aspPubKey],
});

const config: EscrowArkConfig = {
  aspPubKey,
  exitTimelock,
  serverUnrollScript,
  dust: 330n,
};

const funding = { txid: "11".repeat(32), vout: 0, valueSats: 100_000 };
const outputs = {
  buyerArkAddress: arkAddress(10),
  buyerAmountSats: 99_000,
  feeArkAddress: arkAddress(20),
  feeSats: 1_000,
};

describe("escrowArkConfigFromInfo", () => {
  it("derives config fields from ArkInfo", () => {
    const info = {
      signerPubkey: hex.encode(aspPubKey),
      unilateralExitDelay: 144n,
      checkpointTapscript: hex.encode(serverUnrollScript.script),
      dust: 330n,
    } as unknown as ArkInfo;

    const cfg = escrowArkConfigFromInfo(info);
    expect(cfg.aspPubKey).toEqual(aspPubKey);
    expect(cfg.exitTimelock).toEqual({ type: "blocks", value: 144n });
    expect(cfg.dust).toBe(330n);
    expect(cfg.serverUnrollScript.script).toEqual(serverUnrollScript.script);
  });

  it("throws when checkpointTapscript is missing", () => {
    const info = {
      signerPubkey: hex.encode(aspPubKey),
      unilateralExitDelay: 144n,
      dust: 330n,
    } as unknown as ArkInfo;
    expect(() => escrowArkConfigFromInfo(info)).toThrow(/checkpointTapscript/);
  });
});

describe("buildEscrowReleaseTx", () => {
  const expectation = {
    escrowOutpoint: { txid: funding.txid, vout: funding.vout },
    buyerArkAddress: outputs.buyerArkAddress,
    buyerAmountSats: BigInt(outputs.buyerAmountSats),
    feeArkAddress: outputs.feeArkAddress,
    feeAmountSats: BigInt(outputs.feeSats),
  };

  it("produces a release chain that passes verifyReleaseArkTx", () => {
    const escrow = new EscrowVtxoScript(options);
    const built = buildEscrowReleaseTx(escrow, funding, outputs, config);

    expect(built.checkpoints.length).toBe(1);
    // The ark-tx carries buyer + fee + the P2A anchor.
    expect(built.arkTx.outputsLength).toBe(3);
    expect(() => verifyReleaseArkTx(built, expectation)).not.toThrow();
  });

  it("verifyReleaseArkTx rejects a tampered payout amount", () => {
    const escrow = new EscrowVtxoScript(options);
    const built = buildEscrowReleaseTx(escrow, funding, outputs, config);

    expect(() =>
      verifyReleaseArkTx(built, {
        ...expectation,
        buyerAmountSats: BigInt(outputs.buyerAmountSats + 1),
      }),
    ).toThrow();
  });

  it("verifyReleaseArkTx rejects a release that does not spend the funding outpoint", () => {
    const escrow = new EscrowVtxoScript(options);
    const built = buildEscrowReleaseTx(escrow, funding, outputs, config);

    expect(() =>
      verifyReleaseArkTx(built, {
        ...expectation,
        escrowOutpoint: { txid: "22".repeat(32), vout: 0 },
      }),
    ).toThrow(/does not spend funding/);
  });

  it("is deterministic across builds", () => {
    const a = buildEscrowReleaseTx(
      new EscrowVtxoScript(options),
      funding,
      outputs,
      config,
    );
    const b = buildEscrowReleaseTx(
      new EscrowVtxoScript(options),
      funding,
      outputs,
      config,
    );
    expect(hex.encode(a.arkTx.toPSBT())).toBe(hex.encode(b.arkTx.toPSBT()));
  });
});

describe("signEscrowReleaseInPlace", () => {
  it("is deterministic for the same key (stable signatures across rounds)", () => {
    const a = buildEscrowReleaseTx(
      new EscrowVtxoScript(options),
      funding,
      outputs,
      config,
    );
    const b = buildEscrowReleaseTx(
      new EscrowVtxoScript(options),
      funding,
      outputs,
      config,
    );

    const arbiterSk = new Uint8Array(32);
    arbiterSk[31] = 2; // matches arbiterPubKey = xOnlyPubKey(2)
    signEscrowReleaseInPlace(a, arbiterSk);
    signEscrowReleaseInPlace(b, arbiterSk);

    expect(hex.encode(a.arkTx.toPSBT())).toBe(hex.encode(b.arkTx.toPSBT()));
  });
});
