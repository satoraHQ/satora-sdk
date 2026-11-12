import {
  buildOffchainTx,
  CSVMultisigTapscript,
  SingleKey,
  Transaction,
} from "@arkade-os/sdk";
import { base64, hex } from "@scure/base";
import { describe, expect, it, vi } from "vitest";
import {
  type OffchainSubmitProvider,
  submitOffchainTxOneShot,
} from "../src/arkade-submit.js";
import { StrictVhtlcScript } from "../src/strict-vhtlc.js";

const user = SingleKey.fromHex("01".repeat(32));
const receiver = SingleKey.fromHex("02".repeat(32));
const server = SingleKey.fromHex("03".repeat(32));

async function buildRefund() {
  const vhtlc = new StrictVhtlcScript({
    sender: await user.xOnlyPublicKey(),
    receiver: await receiver.xOnlyPublicKey(),
    server: await server.xOnlyPublicKey(),
    preimageHash: new Uint8Array(20).fill(7),
    refundLocktime: 100n,
    unilateralClaimDelay: { type: "blocks", value: 10n },
    unilateralRefundDelay: { type: "blocks", value: 11n },
    unilateralRefundWithoutReceiverDelay: { type: "blocks", value: 12n },
  });
  const serverUnroll = CSVMultisigTapscript.decode(
    CSVMultisigTapscript.encode({
      pubkeys: [await server.xOnlyPublicKey()],
      timelock: { type: "blocks", value: 20n },
    }).script,
  );
  const inputs = [0, 1].map((vout) => ({
    txid: "aa".repeat(32),
    vout,
    value: 10_000,
    tapTree: vhtlc.encode(),
    tapLeafScript: vhtlc.refundWithoutReceiver(),
  }));
  const { arkTx, checkpoints } = buildOffchainTx(
    inputs,
    [{ script: vhtlc.pkScript, amount: 20_000n }],
    serverUnroll,
  );
  const signedArkTx = await user.sign(arkTx);
  const userSigned = await Promise.all(checkpoints.map((c) => user.sign(c)));
  return { arkTx: signedArkTx, checkpoints, userSigned };
}

const decode = (b64: string) => Transaction.fromPSBT(base64.decode(b64));
const encode = (tx: Transaction) => base64.encode(tx.toPSBT());
const sigCount = (tx: Transaction) => tx.getInput(0).tapScriptSig?.length ?? 0;

describe("submitOffchainTxOneShot", () => {
  it("skips finalizeTx when arkd finalized on submit", async () => {
    const { arkTx, checkpoints, userSigned } = await buildRefund();
    const provider: OffchainSubmitProvider = {
      // One-shot arkd: countersigns what we sent, keeping our sigs.
      submitTx: vi.fn(async (_ark: string, cps: string[]) => ({
        arkTxid: arkTx.id,
        finalArkTx: encode(arkTx),
        signedCheckpointTxs: await Promise.all(
          cps.map(async (c) => encode(await server.sign(decode(c), [0]))),
        ),
      })),
      finalizeTx: vi.fn(),
    };

    const res = await submitOffchainTxOneShot(
      provider,
      arkTx,
      checkpoints,
      userSigned,
    );

    expect(res).toEqual({ arkTxid: arkTx.id, mode: "one-shot" });
    const sent = vi.mocked(provider.submitTx).mock.calls[0][1].map(decode);
    expect(sent.every((c) => sigCount(c) === 1)).toBe(true);
    expect(provider.finalizeTx).not.toHaveBeenCalled();
  });

  it("merges our sigs and finalizes when arkd is too old for one-shot", async () => {
    const { arkTx, checkpoints, userSigned } = await buildRefund();
    const provider: OffchainSubmitProvider = {
      // Old arkd: strips incoming sigs, returns server-signed checkpoints
      // in reverse order.
      submitTx: vi.fn(async () => ({
        arkTxid: arkTx.id,
        finalArkTx: encode(arkTx),
        signedCheckpointTxs: await Promise.all(
          [...checkpoints]
            .reverse()
            .map(async (c) => encode(await server.sign(c, [0]))),
        ),
      })),
      finalizeTx: vi.fn(async () => {}),
    };

    const res = await submitOffchainTxOneShot(
      provider,
      arkTx,
      checkpoints,
      userSigned,
    );

    expect(res).toEqual({ arkTxid: arkTx.id, mode: "two-step" });
    const [txid, finals] = vi.mocked(provider.finalizeTx).mock.calls[0];
    expect(txid).toBe(arkTx.id);
    const finalTxs = finals.map(decode);
    expect(finalTxs.map((c) => c.id)).toEqual(
      [...checkpoints].reverse().map((c) => c.id),
    );
    const userPk = hex.encode(await user.xOnlyPublicKey());
    for (const cp of finalTxs) {
      const pubkeys = (cp.getInput(0).tapScriptSig ?? []).map(([k]) =>
        hex.encode(k.pubKey),
      );
      expect(pubkeys).toHaveLength(2);
      expect(pubkeys).toContain(userPk);
    }
  });

  it("rejects a response for a different ark tx", async () => {
    const { arkTx, checkpoints, userSigned } = await buildRefund();
    const provider: OffchainSubmitProvider = {
      submitTx: vi.fn(async (_a: string, cps: string[]) => ({
        arkTxid: "bb".repeat(32),
        finalArkTx: encode(arkTx),
        signedCheckpointTxs: cps,
      })),
      finalizeTx: vi.fn(),
    };

    await expect(
      submitOffchainTxOneShot(provider, arkTx, checkpoints, userSigned),
    ).rejects.toThrow();
    expect(provider.finalizeTx).not.toHaveBeenCalled();
  });
});
