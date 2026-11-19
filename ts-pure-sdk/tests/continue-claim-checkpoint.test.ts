import {
  ArkAddress,
  buildOffchainTx,
  ConditionWitness,
  CSVMultisigTapscript,
  getArkPsbtFields,
  RestIndexerProvider,
  RestOperatorProvider,
  SingleKey,
  Transaction,
} from "@arkade-os/sdk";
import { ripemd160 } from "@noble/hashes/legacy.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { base64, hex } from "@scure/base";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  Client,
  InMemorySwapStorage,
  InMemoryWalletStorage,
} from "../src/index.js";
import { SWAP_STORAGE_VERSION } from "../src/storage/types.js";
import {
  ARKADE_HTLC_SCRIPT_VERSION_STRICT,
  StrictVhtlcScript,
} from "../src/strict-vhtlc.js";

const swapId = "c5c4baf4-a7ad-41ce-b74c-2632053fd804";
const preimage = new Uint8Array(32).fill(7);

afterEach(() => vi.restoreAllMocks());

async function pendingClaim(vtxoStatus: "spent" | "spendable" = "spent") {
  const secretKey = "01".repeat(32);
  const user = SingleKey.fromHex(secretKey);
  const sender = SingleKey.fromHex("02".repeat(32));
  const operator = SingleKey.fromHex("03".repeat(32));
  const receiverKey = await user.xOnlyPublicKey();
  const senderKey = await sender.xOnlyPublicKey();
  const operatorKey = await operator.xOnlyPublicKey();
  const preimageHash = sha256(preimage);
  const delay = { type: "seconds" as const, value: 512n };
  const refundLocktime = 1893456000n;
  const vhtlc = new StrictVhtlcScript({
    receiver: receiverKey,
    sender: senderKey,
    server: operatorKey,
    preimageHash: ripemd160(preimageHash),
    refundLocktime,
    unilateralClaimDelay: delay,
    unilateralRefundDelay: delay,
    unilateralRefundWithoutReceiverDelay: delay,
  });
  const serverUnroll = CSVMultisigTapscript.encode({
    pubkeys: [operatorKey],
    timelock: { type: "blocks", value: 20n },
  });
  const destination = new ArkAddress(operatorKey, receiverKey, "tark");
  const otherDestination = new ArkAddress(operatorKey, senderKey, "tark");
  const vtxo = {
    txid: "aa".repeat(32),
    vout: 0,
    value: 10_000,
    script: hex.encode(vhtlc.pkScript),
    createdAt: new Date(),
    isUnrolled: false,
    status: { confirmed: true },
  };
  const { arkTx, checkpoints } = buildOffchainTx(
    [{ ...vtxo, tapTree: vhtlc.encode(), tapLeafScript: vhtlc.claim() }],
    [{ script: destination.pkScript, amount: BigInt(vtxo.value) }],
    serverUnroll,
  );
  const signedArkTx = await operator.sign(await user.sign(arkTx));
  const signedCheckpoint = await operator.sign(checkpoints[0], [0]);
  const pending = {
    arkTxid: signedArkTx.id,
    finalArkTx: base64.encode(signedArkTx.toPSBT()),
    signedCheckpointTxs: [base64.encode(signedCheckpoint.toPSBT())],
  };

  const storage = new InMemorySwapStorage();
  const client = await Client.builder()
    .withBaseUrl("https://api.test")
    .withMnemonic(
      "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    )
    .withSignerStorage(new InMemoryWalletStorage())
    .withSwapStorage(storage)
    .build();
  const token = {
    token_id: "btc",
    symbol: "BTC",
    name: "Bitcoin",
    decimals: 8,
  };
  const now = Date.now();
  await storage.store({
    version: SWAP_STORAGE_VERSION,
    swapId,
    keyIndex: 0,
    publicKey: hex.encode(receiverKey),
    secretKey,
    preimage: hex.encode(preimage),
    preimageHash: hex.encode(preimageHash),
    storedAt: now,
    updatedAt: now,
    response: {
      id: swapId,
      direction: "lightning_to_arkade",
      status: "clientredeeming",
      network: "signet",
      arkade_htlc_script_version: ARKADE_HTLC_SCRIPT_VERSION_STRICT,
      arkade_server_pk: hex.encode(operatorKey),
      arkade_vhtlc_address: vhtlc.address("tark", operatorKey).encode(),
      receiver_pk: hex.encode(receiverKey),
      sender_pk: hex.encode(senderKey),
      hash_lock: `0x${hex.encode(preimageHash)}`,
      vhtlc_refund_locktime: Number(refundLocktime),
      unilateral_claim_delay: Number(delay.value),
      unilateral_refund_delay: Number(delay.value),
      unilateral_refund_without_receiver_delay: Number(delay.value),
      target_arkade_address: destination.encode(),
      bolt11_invoice: "unused",
      invoice_expires_at: Number(refundLocktime) - 3600,
      created_at: new Date(now).toISOString(),
      source_amount: "10000",
      target_amount: "10000",
      fee_sats: 0,
      network_fee_sats: 0,
      source_token: { ...token, chain: "Lightning" },
      target_token: { ...token, chain: "Arkade" },
    },
  });

  vi.spyOn(RestIndexerProvider.prototype, "getVtxos").mockImplementation(
    async (options) => {
      const includeVtxo = options?.spendableOnly
        ? vtxoStatus === "spendable"
        : options?.spentOnly
          ? vtxoStatus === "spent"
          : !options?.recoverableOnly;
      return { vtxos: includeVtxo ? [vtxo] : [] };
    },
  );
  vi.spyOn(RestOperatorProvider.prototype, "getPendingTxs").mockResolvedValue([
    pending,
  ]);
  const finalize = vi
    .spyOn(RestOperatorProvider.prototype, "finalizeTx")
    .mockResolvedValue(undefined);
  const submit = vi
    .spyOn(RestOperatorProvider.prototype, "submitTx")
    .mockRejectedValue(new Error("Unexpected new submission"));
  return {
    client,
    destination,
    otherDestination,
    pending,
    receiverKey,
    finalize,
    submit,
  };
}

describe("resuming an Arkade claim", () => {
  it("signs the pending Taproot checkpoint and finalizes without another submission", async () => {
    const { client, destination, pending, receiverKey, finalize, submit } =
      await pendingClaim();

    await expect(
      client.claimArkade(swapId, { destinationAddress: destination.encode() }),
    ).resolves.toMatchObject({ success: true, txId: pending.arkTxid });

    expect(submit).not.toHaveBeenCalled();
    expect(finalize).toHaveBeenCalledExactlyOnceWith(pending.arkTxid, [
      expect.any(String),
    ]);
    const checkpoint = Transaction.fromPSBT(
      base64.decode(finalize.mock.calls[0][1][0]),
    );
    expect(getArkPsbtFields(checkpoint, 0, ConditionWitness)).toEqual([
      [preimage],
    ]);
    expect(
      checkpoint.getInput(0).tapScriptSig?.map(([key]) => key.pubKey),
    ).toContainEqual(receiverKey);
  });

  it.each([
    "spent",
    "spendable",
  ] as const)("rejects a changed destination when VTXOs look %s", async (vtxoStatus) => {
    const { client, otherDestination, finalize, submit } =
      await pendingClaim(vtxoStatus);

    await expect(
      client.claimArkade(swapId, {
        destinationAddress: otherDestination.encode(),
      }),
    ).resolves.toMatchObject({
      success: false,
      message: expect.stringContaining(
        "does not pay the requested destination",
      ),
    });
    expect(finalize).not.toHaveBeenCalled();
    expect(submit).not.toHaveBeenCalled();
  });

  it("surfaces an operator finalization failure through claimArkade", async () => {
    const { client, destination, finalize } = await pendingClaim();
    finalize.mockRejectedValueOnce(new Error("Operator could not finalize"));

    await expect(
      client.claimArkade(swapId, { destinationAddress: destination.encode() }),
    ).resolves.toEqual({
      success: false,
      message: "Operator could not finalize",
    });
  });

  it("resumes the original claim by swap ID alone", async () => {
    const { client, pending, finalize, submit } = await pendingClaim();
    await expect(client.continueArkadeClaimSwap(swapId)).resolves.toMatchObject(
      {
        success: true,
        txId: pending.arkTxid,
      },
    );
    expect(finalize).toHaveBeenCalledTimes(1);
    expect(submit).not.toHaveBeenCalled();
  });
});
