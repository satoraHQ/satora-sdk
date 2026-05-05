/**
 * Collaborative refund for Arkade-to-Lightning swaps.
 *
 * When the Lightning payment fails, the user can collaboratively refund the
 * VHTLC using the `refund` script leaf (3-of-3: sender + receiver + Arkade server).
 *
 * The client signs as sender, POSTs to the Lendaswap collab-refund endpoint
 * for the receiver cosignature, then submits to Arkade for the server signature.
 *
 * Only the offchain (spendable VTXOs) flow is supported — the delegate flow
 * requires the receiver to cosign intent+forfeits which isn't supported.
 */

import {
  ArkAddress,
  type ArkProvider,
  buildOffchainTx,
  CSVMultisigTapscript,
  type IndexerProvider,
  RestArkProvider,
  RestIndexerProvider,
  SingleKey,
  Transaction,
  VHTLC,
} from "@arkade-os/sdk";
import { ripemd160 } from "@noble/hashes/legacy.js";
import { base64, hex } from "@scure/base";

import type { ApiClient } from "../api/client.js";
import {
  getNetworkHrp,
  getNetworkName,
  resolveArkadeServerUrlByName,
} from "../arkade-network.js";
import { createSdkLogger, type Logger, type LogLevel } from "../logging.js";

function secondsToTimelock(
  seconds: number,
): VHTLC.Options["unilateralClaimDelay"] {
  return { type: "seconds" as const, value: BigInt(seconds) };
}

function parseXOnlyPubKey(pubKeyHex: string): Uint8Array {
  const bytes = hex.decode(pubKeyHex);
  if (bytes.length === 33) return bytes.slice(1);
  if (bytes.length === 32) return bytes;
  throw new Error(
    `Invalid public key length: expected 32 or 33, got ${bytes.length}`,
  );
}

function parsePreimageHash(hashLock: string): Uint8Array {
  const hashLockBytes = hex.decode(hashLock);
  if (hashLockBytes.length === 32) {
    return ripemd160(hashLockBytes);
  }
  if (hashLockBytes.length === 20) {
    return hashLockBytes;
  }
  throw new Error(
    `Invalid hash lock length: expected 20 or 32, got ${hashLockBytes.length}`,
  );
}

function formatApiError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object") {
    if (
      "error" in error &&
      typeof (error as { error: unknown }).error === "string"
    ) {
      return (error as { error: string }).error;
    }
    try {
      return JSON.stringify(error);
    } catch {
      // fall through
    }
  }
  return String(error);
}

/** Parameters for collaborative refund of an Arkade-to-Lightning VHTLC. */
export interface CollabRefundArkadeToLightningParams {
  /** User's secret key (sender in the VHTLC). */
  userSecretKey: string;
  /** User's public key (x-only, hex). */
  userPubKey: string;
  /** Receiver's claim public key (x-only, hex). */
  receiverPubKey: string;
  /** Arkade server public key. */
  arkadeServerPubKey: string;
  /** Hash lock (preimage hash, hex). */
  hashLock: string;
  /** VHTLC address to refund from. */
  vhtlcAddress: string;
  /** VHTLC refund locktime (CLTV). */
  refundLocktime: number;
  /** Unilateral claim delay in seconds. */
  unilateralClaimDelay: number;
  /** Unilateral refund delay in seconds. */
  unilateralRefundDelay: number;
  /** Unilateral refund without receiver delay in seconds. */
  unilateralRefundWithoutReceiverDelay: number;
  /** Destination Ark address for the refunded funds. */
  destinationAddress: string;
  /** Bitcoin network name. */
  network: string;
  /** Arkade server URL (optional, defaults by network). */
  arkadeServerUrl?: string;
  /** Swap ID (used for the collab-refund endpoint). */
  swapId: string;
  /** API client for Lendaswap backend. */
  apiClient: ApiClient;
  /** Optional logger sink. Silent by default. */
  logger?: Logger;
  /** Minimum log level to emit. Defaults to `silent`. */
  logLevel?: LogLevel;
}

export interface CollabRefundArkadeToLightningResult {
  txId: string;
  refundAmount: bigint;
}

/**
 * Collaborative refund of an Arkade-to-Lightning VHTLC via spendable VTXOs (offchain send).
 *
 * Uses the `refund` script leaf (3-of-3: sender + receiver + Arkade server).
 * The client signs as sender, POSTs to `/api/swap/arkade-lightning/{id}/collab-refund`
 * for the receiver cosignature, then submits to Arkade for the server signature.
 */
export async function collabRefundArkadeToLightningOffchain(
  params: CollabRefundArkadeToLightningParams,
): Promise<CollabRefundArkadeToLightningResult> {
  const logger = createSdkLogger(params).child({
    module: "refund/collab-arkade-lightning",
    operation: "arkade_to_lightning.collab_refund",
    swapId: params.swapId,
    data: {
      vhtlcAddress: params.vhtlcAddress,
      network: params.network,
      destinationAddress: params.destinationAddress,
    },
  });

  logger.info({
    event: "arkade_to_lightning.collab_refund.start",
    message: "Starting collaborative Arkade-to-Lightning refund",
  });

  // 1. Build the VHTLC script and verify address
  const userPkBytes = parseXOnlyPubKey(params.userPubKey);
  const receiverPkBytes = parseXOnlyPubKey(params.receiverPubKey);
  const serverPkBytes = parseXOnlyPubKey(params.arkadeServerPubKey);
  const preimageHashBytes = parsePreimageHash(params.hashLock);

  const networkName = getNetworkName(params.network);

  const vhtlc = new VHTLC.Script({
    sender: userPkBytes,
    receiver: receiverPkBytes,
    server: serverPkBytes,
    preimageHash: preimageHashBytes,
    refundLocktime: BigInt(params.refundLocktime),
    unilateralClaimDelay: secondsToTimelock(params.unilateralClaimDelay),
    unilateralRefundDelay: secondsToTimelock(params.unilateralRefundDelay),
    unilateralRefundWithoutReceiverDelay: secondsToTimelock(
      params.unilateralRefundWithoutReceiverDelay,
    ),
  });

  const hrp = getNetworkHrp(networkName);
  const computedAddress = vhtlc.address(hrp, serverPkBytes).encode();

  if (computedAddress !== params.vhtlcAddress) {
    logger.error({
      event: "arkade_to_lightning.collab_refund.address_mismatch",
      message: "Computed VHTLC address does not match expected address",
      data: {
        sender: hex.encode(userPkBytes),
        receiver: hex.encode(receiverPkBytes),
        server: hex.encode(serverPkBytes),
        preimageHash: hex.encode(preimageHashBytes),
        refundLocktime: params.refundLocktime,
        unilateralClaimDelay: params.unilateralClaimDelay,
        unilateralRefundDelay: params.unilateralRefundDelay,
        unilateralRefundWithoutReceiverDelay:
          params.unilateralRefundWithoutReceiverDelay,
        computedAddress,
        expectedAddress: params.vhtlcAddress,
        network: networkName,
      },
    });

    throw new Error(
      `VHTLC address mismatch: computed ${computedAddress}, expected ${params.vhtlcAddress}. ` +
        `This may indicate corrupted swap data.`,
    );
  }

  // 2. Connect to Arkade and fetch VTXOs
  const serverUrl = resolveArkadeServerUrlByName(
    networkName,
    params.arkadeServerUrl,
  );

  const arkProvider: ArkProvider = new RestArkProvider(serverUrl);
  const indexerProvider: IndexerProvider = new RestIndexerProvider(serverUrl);
  const serverInfo = await arkProvider.getInfo();

  const vhtlcPkScript = hex.encode(vhtlc.pkScript);

  logger.debug({
    event: "arkade_to_lightning.collab_refund.query_vtxos",
    message: "Querying VTXOs for VHTLC script",
    data: { vhtlcPkScript },
  });

  const { vtxos } = await indexerProvider.getVtxos({
    scripts: [vhtlcPkScript],
    spendableOnly: true,
  });

  logger.info({
    event: "arkade_to_lightning.collab_refund.vtxos_found",
    message: "Found spendable VTXOs for collaborative refund",
    data: {
      vtxoCount: vtxos.length,
      vtxos: vtxos.map((v) => ({
        txid: v.txid,
        vout: v.vout,
        value: v.value,
      })),
    },
  });

  if (vtxos.length === 0) {
    throw new Error(
      "No spendable VTXOs found at the VHTLC address. " +
        "The VTXO may have already been spent or may be in a recoverable (non-spendable) state.",
    );
  }

  const totalAmount = vtxos.reduce((acc, v) => acc + BigInt(v.value), 0n);
  if (totalAmount === 0n) {
    throw new Error("Total VTXO amount is zero");
  }

  // 3. Build offchain transaction
  const tapTree = vhtlc.encode();
  const refundLeafScript = vhtlc.refund();

  const checkpointTapscript = CSVMultisigTapscript.decode(
    hex.decode(serverInfo.checkpointTapscript),
  );

  const inputs = vtxos.map((v) => ({
    txid: v.txid,
    vout: v.vout,
    value: v.value,
    tapTree,
    tapLeafScript: refundLeafScript,
  }));

  const destination = ArkAddress.decode(params.destinationAddress);
  const outputs = [{ script: destination.pkScript, amount: totalAmount }];

  const { arkTx, checkpoints } = buildOffchainTx(
    inputs,
    outputs,
    checkpointTapscript,
  );

  // 4. Snapshot unsigned checkpoint BEFORE signing
  const unsignedCheckpointPsbts = checkpoints.map((cp) =>
    base64.encode(cp.toPSBT()),
  );

  if (unsignedCheckpointPsbts.length === 0) {
    throw new Error("No checkpoint PSBTs generated");
  }

  logger.debug({
    event: "arkade_to_lightning.collab_refund.offchain_tx_built",
    message: "Built offchain transaction for collaborative refund",
    data: {
      totalAmount,
      numInputs: inputs.length,
      destination: params.destinationAddress,
      numCheckpoints: checkpoints.length,
    },
  });

  // 5. Sign ark_tx as sender
  const signer = SingleKey.fromHex(params.userSecretKey);
  const signedArkTx = await signer.sign(arkTx);

  logger.debug({
    event: "arkade_to_lightning.collab_refund.post_backend",
    message: "Posting collaborative refund transaction to backend",
    data: {
      arkTxPsbtLen: base64.encode(signedArkTx.toPSBT()).length,
      checkpointPsbtLen: unsignedCheckpointPsbts[0].length,
    },
  });

  // 6. POST to Lendaswap for receiver cosignature
  const proxyRes = await params.apiClient.POST(
    "/api/swap/arkade-lightning/{id}/collab-refund",
    {
      params: { path: { id: params.swapId } },
      body: {
        ark_tx: base64.encode(signedArkTx.toPSBT()),
        checkpoint: unsignedCheckpointPsbts[0],
      },
    },
  );

  if (!proxyRes.data) {
    throw new Error(
      `Collaborative refund failed: ${formatApiError(proxyRes.error)}. ` +
        `You can still refund after the VHTLC locktime expires.`,
    );
  }

  // 7. Save receiver's checkpoint sigs before submitting to Arkade
  //    (Arkade strips incoming sigs and only returns its own)
  const receiverCheckpointTx = Transaction.fromPSBT(
    base64.decode(proxyRes.data.checkpoint),
  );
  type TapScriptSigEntry = [
    { pubKey: Uint8Array; leafHash: Uint8Array },
    Uint8Array,
  ];
  const receiverCheckpointSigs: TapScriptSigEntry[][] = [];
  for (let i = 0; i < receiverCheckpointTx.inputsLength; i++) {
    const input = receiverCheckpointTx.getInput(i);
    const sigs: TapScriptSigEntry[] = [];
    if (input.tapScriptSig) {
      for (const [key, sig] of input.tapScriptSig) {
        sigs.push([
          {
            pubKey: Uint8Array.from(key.pubKey),
            leafHash: Uint8Array.from(key.leafHash),
          },
          Uint8Array.from(sig),
        ]);
      }
    }
    receiverCheckpointSigs.push(sigs);
  }

  // 8. Submit cosigned ark_tx + checkpoint to Arkade for server signature
  const submitRes = await arkProvider.submitTx(proxyRes.data.ark_tx, [
    proxyRes.data.checkpoint,
  ]);

  // 9. Merge receiver's checkpoint sigs back, then sign as sender
  const finalCheckpoints = await Promise.all(
    submitRes.signedCheckpointTxs.map(async (cp) => {
      const tx = Transaction.fromPSBT(base64.decode(cp));

      for (
        let i = 0;
        i < tx.inputsLength && i < receiverCheckpointSigs.length;
        i++
      ) {
        for (const [key, signature] of receiverCheckpointSigs[i]) {
          tx.updateInput(i, {
            tapScriptSig: [[key, signature]],
          });
        }
      }

      const signed = await signer.sign(tx);
      return base64.encode(signed.toPSBT());
    }),
  );

  // 10. Finalize
  await arkProvider.finalizeTx(submitRes.arkTxid, finalCheckpoints);

  return { txId: submitRes.arkTxid, refundAmount: totalAmount };
}
