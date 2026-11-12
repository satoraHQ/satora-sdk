import {
  CSVMultisigTapscript,
  networks,
  type OperatorInfo,
  Transaction,
} from "@arkade-os/sdk";
import { schnorr } from "@noble/curves/secp256k1.js";
import { base64, hex } from "@scure/base";
import { describe, expect, it, vi } from "vitest";
import { type EscrowScriptOptions, EscrowVtxoScript } from "./escrow-script.js";
import {
  buildEscrowReleaseTx,
  type EscrowArkConfig,
  escrowArkConfigFromInfo,
  signEscrowReleaseInPlace,
  submitAndFinalizeEscrowRelease,
} from "./release.js";
import { verifyReleaseArkTx } from "./verify.js";

function xOnlyPubKey(seed: number): Uint8Array {
  const sk = new Uint8Array(32);
  sk[31] = seed;
  return schnorr.getPublicKey(sk);
}

const sellerPubKey = xOnlyPubKey(1);
const arbiterPubKey = xOnlyPubKey(2);
const arkadeServerPubKey = xOnlyPubKey(3);
const exitTimelock = { type: "blocks", value: 4320n } as const;
const network = networks.regtest;

const options: EscrowScriptOptions = {
  sellerPubKey,
  arbiterPubKey,
  arkadeServerPubKey,
  exitTimelock,
};

/** A valid, distinct Arkade address derived from a throwaway escrow script. */
function arkAddress(seed: number): string {
  return new EscrowVtxoScript({
    sellerPubKey: xOnlyPubKey(seed),
    arbiterPubKey: xOnlyPubKey(seed + 1),
    arkadeServerPubKey,
    exitTimelock,
  }).arkAddress(network);
}

const serverUnrollScript = CSVMultisigTapscript.encode({
  timelock: exitTimelock,
  pubkeys: [arkadeServerPubKey],
});

const config: EscrowArkConfig = {
  arkadeServerPubKey,
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
  it("derives config fields from OperatorInfo", () => {
    const info = {
      signerPubkey: hex.encode(arkadeServerPubKey),
      unilateralExitDelay: 144n,
      checkpointTapscript: hex.encode(serverUnrollScript.script),
      dust: 330n,
    } as unknown as OperatorInfo;

    const cfg = escrowArkConfigFromInfo(info);
    expect(cfg.arkadeServerPubKey).toEqual(arkadeServerPubKey);
    expect(cfg.exitTimelock).toEqual({ type: "blocks", value: 144n });
    expect(cfg.dust).toBe(330n);
    expect(cfg.serverUnrollScript.script).toEqual(serverUnrollScript.script);
  });

  it("throws when checkpointTapscript is missing", () => {
    const info = {
      signerPubkey: hex.encode(arkadeServerPubKey),
      unilateralExitDelay: 144n,
      dust: 330n,
    } as unknown as OperatorInfo;
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
    // The Arkade transaction carries buyer + fee + the P2A anchor.
    expect(built.arkTx.outputsLength).toBe(3);
    expect(() => verifyReleaseArkTx(built, expectation)).not.toThrow();
  });

  it("omits the fee output for a zero-fee release and still verifies", () => {
    const escrow = new EscrowVtxoScript(options);
    // The buyer takes the whole funding so inputs and outputs still balance.
    const zeroFeeOutputs = {
      ...outputs,
      buyerAmountSats: funding.valueSats,
      feeSats: 0,
    };
    const built = buildEscrowReleaseTx(escrow, funding, zeroFeeOutputs, config);

    // Only buyer + the P2A anchor; no fee output.
    expect(built.arkTx.outputsLength).toBe(2);
    expect(() =>
      verifyReleaseArkTx(built, {
        ...expectation,
        buyerAmountSats: BigInt(funding.valueSats),
        feeAmountSats: 0n,
      }),
    ).not.toThrow();
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

  it("verifyReleaseArkTx rejects a duplicate buyer output", () => {
    const escrow = new EscrowVtxoScript(options);
    const built = buildEscrowReleaseTx(escrow, funding, outputs, config);
    built.arkTx.addOutput(built.arkTx.getOutput(0));

    expect(() => verifyReleaseArkTx(built, expectation)).toThrow(
      /expected exactly 1 buyer output/,
    );
  });

  it("verifyReleaseArkTx rejects a duplicate fee output", () => {
    const escrow = new EscrowVtxoScript(options);
    const built = buildEscrowReleaseTx(escrow, funding, outputs, config);
    built.arkTx.addOutput(built.arkTx.getOutput(1));

    expect(() => verifyReleaseArkTx(built, expectation)).toThrow(
      /expected exactly 1 fee output/,
    );
  });

  it("verifyReleaseArkTx rejects a duplicate P2A anchor output", () => {
    const escrow = new EscrowVtxoScript(options);
    const built = buildEscrowReleaseTx(escrow, funding, outputs, config);
    built.arkTx.addOutput(built.arkTx.getOutput(2));

    expect(() => verifyReleaseArkTx(built, expectation)).toThrow(
      /expected exactly 1 P2A anchor output/,
    );
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

describe("submitAndFinalizeEscrowRelease", () => {
  const sk = (seed: number) => {
    const key = new Uint8Array(32);
    key[31] = seed;
    return key;
  };
  const decode = (b64: string) => Transaction.fromPSBT(base64.decode(b64));
  const encode = (tx: Transaction) => base64.encode(tx.toPSBT());
  const serverSign = (tx: Transaction) => {
    tx.signIdx(sk(3), 0);
    return encode(tx);
  };

  function signedRelease() {
    const built = buildEscrowReleaseTx(
      new EscrowVtxoScript(options),
      funding,
      outputs,
      config,
    );
    const unsigned = built.checkpoints.map((c) => c.clone());
    signEscrowReleaseInPlace(built, sk(1));
    signEscrowReleaseInPlace(built, sk(2));
    return { built, unsigned };
  }

  it("sends the user-signed checkpoints and skips finalize on a one-shot server", async () => {
    const { built, unsigned } = signedRelease();
    const provider = {
      submitTx: vi.fn(async (_ark: string, cps: string[]) => ({
        arkTxid: built.arkTx.id,
        finalArkTx: encode(built.arkTx),
        signedCheckpointTxs: cps.map((c) => serverSign(decode(c))),
      })),
      finalizeTx: vi.fn(),
    };

    const txid = await submitAndFinalizeEscrowRelease(
      provider,
      built.arkTx,
      built.checkpoints,
      unsigned,
    );

    expect(txid).toBe(built.arkTx.id);
    const sent = provider.submitTx.mock.calls[0][1].map(decode);
    expect(sent.map((c) => c.getInput(0).tapScriptSig?.length)).toEqual(
      sent.map(() => 2),
    );
    expect(provider.finalizeTx).not.toHaveBeenCalled();
  });

  it("merges user sigs and finalizes on an older server", async () => {
    const { built, unsigned } = signedRelease();
    const provider = {
      submitTx: vi.fn(async () => ({
        arkTxid: built.arkTx.id,
        finalArkTx: encode(built.arkTx),
        signedCheckpointTxs: unsigned.map((c) => serverSign(c.clone())),
      })),
      finalizeTx: vi.fn(async () => {}),
    };

    const txid = await submitAndFinalizeEscrowRelease(
      provider,
      built.arkTx,
      built.checkpoints,
      unsigned,
    );

    expect(txid).toBe(built.arkTx.id);
    const [finalTxid, finals] = provider.finalizeTx.mock
      .calls[0] as unknown as [string, string[]];
    expect(finalTxid).toBe(built.arkTx.id);
    for (const cp of finals.map(decode)) {
      expect(cp.getInput(0).tapScriptSig).toHaveLength(3);
    }
  });
});
