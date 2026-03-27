/**
 * Collaborative refund for Arkade-to-EVM swaps.
 *
 * Unlike the `refundWithoutReceiver` path (which requires the CLTV locktime
 * to expire), the collaborative refund uses the `refund` script leaf — a
 * 3-of-3 multisig between sender (client), receiver (lendaswap), and the
 * Arkade server. Lendaswap cosigns via the backend collab-refund endpoint,
 * making the refund instant (no locktime wait).
 *
 * Two flows handle the two VTXO states:
 * - **Spendable VTXOs**: offchain send (submitTx / finalizeTx)
 * - **Recoverable / mixed VTXOs**: delegate batch (intent + forfeits → settle)
 */

import {
  ArkAddress,
  type ArkProvider,
  buildOffchainTx,
  CSVMultisigTapscript,
  type IndexerProvider,
  Intent,
  type NetworkName,
  networks,
  RestArkProvider,
  RestIndexerProvider,
  SingleKey,
  setArkPsbtField,
  Transaction,
  VHTLC,
  VtxoTaprootTree,
} from "@arkade-os/sdk";
import { ripemd160 } from "@noble/hashes/legacy.js";
import { base64, hex } from "@scure/base";
import { Address, OutScript, SigHash } from "@scure/btc-signer";

import type { ApiClient } from "../api/client.js";

// P2A is the zero-value anchor output (OP_1 0x4e73)
const P2A_SCRIPT = new Uint8Array([0x51, 0x02, 0x4e, 0x73]);
const P2A = { script: P2A_SCRIPT, amount: 0n };

/** Default Arkade server URL by network */
const DEFAULT_ARKADE_URLS: Record<string, string> = {
  bitcoin: "https://arkade.computer",
  mainnet: "https://arkade.computer",
  signet: "https://mutinynet.arkade.sh",
  mutinynet: "https://mutinynet.arkade.sh",
};

function getNetworkName(network: string): NetworkName {
  switch (network.toLowerCase()) {
    case "mainnet":
    case "bitcoin":
      return "bitcoin";
    case "testnet":
      return "testnet";
    case "signet":
      return "signet";
    case "mutinynet":
      return "mutinynet";
    case "regtest":
      return "regtest";
    default:
      throw new Error(`Unknown network: ${network}`);
  }
}

function getNetworkHrp(networkName: NetworkName): string {
  return networks[networkName].hrp;
}

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

/** Parameters for collaborative refund. */
interface BaseCollabRefundArkadeToEvmParams {
  userSecretKey: string;
  userPubKey: string;
  lendaswapPubKey: string;
  arkadeServerPubKey: string;
  hashLock: string;
  vhtlcAddress: string;
  refundLocktime: number;
  unilateralClaimDelay: number;
  unilateralRefundDelay: number;
  unilateralRefundWithoutReceiverDelay: number;
  destinationAddress: string;
  network: string;
  arkadeServerUrl?: string;
  swapId: string;
  apiClient: ApiClient;
}

export type CollabRefundArkadeToEvmParams = BaseCollabRefundArkadeToEvmParams;

export interface CollabRefundArkadeToEvmResult {
  txId: string;
  refundAmount: bigint;
}

/**
 * Build the VHTLC script and verify the address matches.
 */
function buildRefundScriptVhtlc(params: BaseCollabRefundArkadeToEvmParams) {
  const userPkBytes = parseXOnlyPubKey(params.userPubKey);
  const lendaswapPkBytes = parseXOnlyPubKey(params.lendaswapPubKey);
  const serverPkBytes = parseXOnlyPubKey(params.arkadeServerPubKey);
  const preimageHashBytes = parsePreimageHash(params.hashLock);

  const networkName = getNetworkName(params.network);

  const vhtlc = new VHTLC.Script({
    sender: userPkBytes,
    receiver: lendaswapPkBytes,
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
    throw new Error(
      `VHTLC address mismatch: computed ${computedAddress}, expected ${params.vhtlcAddress}`,
    );
  }

  return { vhtlc, networkName };
}

/**
 * Collaborative refund for spendable VTXOs.
 *
 * Uses the `refund` script leaf (3-of-3: sender + receiver + server).
 * The client signs as sender, POSTs to `/collab-refund` for lendaswap's
 * receiver signature, then submits to Arkade for the server signature.
 */
export async function collabRefundArkadeToEvmOffchain(
  params: BaseCollabRefundArkadeToEvmParams,
): Promise<CollabRefundArkadeToEvmResult> {
  const { vhtlc, networkName } = buildRefundScriptVhtlc(params);

  const serverUrl = params.arkadeServerUrl ?? DEFAULT_ARKADE_URLS[networkName];
  if (!serverUrl) {
    throw new Error(
      `No Arkade server URL configured for network: ${networkName}`,
    );
  }

  const arkProvider: ArkProvider = new RestArkProvider(serverUrl);
  const indexerProvider: IndexerProvider = new RestIndexerProvider(serverUrl);
  const serverInfo = await arkProvider.getInfo();

  const vhtlcPkScript = hex.encode(vhtlc.pkScript);

  const { vtxos } = await indexerProvider.getVtxos({
    scripts: [vhtlcPkScript],
    spendableOnly: true,
  });

  if (vtxos.length === 0) {
    throw new Error("No spendable VTXOs found at the VHTLC address");
  }

  const totalAmount = vtxos.reduce((acc, v) => acc + BigInt(v.value), 0n);
  if (totalAmount === 0n) {
    throw new Error("Total VTXO amount is zero");
  }

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

  // Snapshot checkpoint PSBTs BEFORE signing — signer.sign() may mutate
  // shared internal state between arkTx and checkpoints.
  const unsignedCheckpointPsbts = checkpoints.map((cp) =>
    base64.encode(cp.toPSBT()),
  );

  const signer = SingleKey.fromHex(params.userSecretKey);
  const signedArkTx = await signer.sign(arkTx);

  // POST to lendaswap backend for receiver cosigning.
  // The backend cosigns both ark_tx (for the 3-of-3 refund leaf) and
  // checkpoint_txs (which also need the receiver signature).
  const collabRes = await params.apiClient.POST(
    "/api/swap/arkade-evm/{id}/collab-refund",
    {
      params: { path: { id: params.swapId } },
      body: {
        ark_tx: base64.encode(signedArkTx.toPSBT()),
        checkpoint_txs: unsignedCheckpointPsbts,
      },
    },
  );

  if (!collabRes.data) {
    throw new Error(
      `Collaborative refund cosign failed: ${formatApiError(collabRes.error)}`,
    );
  }

  // Save the receiver's (lendaswap) tap_script_sigs from each cosigned
  // checkpoint BEFORE submitting to arkd. arkd strips incoming signatures
  // and only returns checkpoints with its own (server) signature.
  // We must merge the receiver sigs back in before client-signing.
  type TapScriptSigEntry = [
    { pubKey: Uint8Array; leafHash: Uint8Array },
    Uint8Array,
  ];
  const receiverCheckpointSigs = collabRes.data.checkpoint_txs.map((cp) => {
    const tx = Transaction.fromPSBT(base64.decode(cp));
    const inputSigs: TapScriptSigEntry[][] = [];
    for (let i = 0; i < tx.inputsLength; i++) {
      const input = tx.getInput(i);
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
      inputSigs.push(sigs);
    }
    return inputSigs;
  });

  // Submit to Arkade with the cosigned ark_tx and the backend-cosigned
  // checkpoints. Arkade adds its server signature to both.
  const submitRes = await arkProvider.submitTx(
    collabRes.data.ark_tx,
    collabRes.data.checkpoint_txs,
  );

  // Finalize: Arkade returns checkpoints with only the server signature.
  // We must merge back the receiver sigs, then add the client (sender)
  // signature to complete the 3-of-3.
  const finalCheckpoints = await Promise.all(
    submitRes.signedCheckpointTxs.map(async (cp, cpIdx) => {
      const tx = Transaction.fromPSBT(base64.decode(cp));

      // Merge receiver tap_script_sigs back into each input
      const savedSigs = receiverCheckpointSigs[cpIdx];
      if (savedSigs) {
        for (let i = 0; i < tx.inputsLength && i < savedSigs.length; i++) {
          for (const [key, signature] of savedSigs[i]) {
            tx.updateInput(i, {
              tapScriptSig: [[key, signature]],
            });
          }
        }
      }

      const signed = await signer.sign(tx);
      return base64.encode(signed.toPSBT());
    }),
  );

  await arkProvider.finalizeTx(submitRes.arkTxid, finalCheckpoints);

  return { txId: submitRes.arkTxid, refundAmount: totalAmount };
}

/**
 * Collaborative refund for recoverable / mixed VTXOs via the delegate flow.
 *
 * Uses the `refund` script leaf. The client signs intent + forfeit PSBTs,
 * POSTs to `/collab-refund-delegate` for receiver cosigning, then settles
 * via `/delegate/settle`.
 */
export async function collabRefundArkadeToEvmDelegate(
  params: BaseCollabRefundArkadeToEvmParams,
): Promise<{ commitmentTxid: string }> {
  const { vhtlc, networkName } = buildRefundScriptVhtlc(params);

  const serverUrl = params.arkadeServerUrl ?? DEFAULT_ARKADE_URLS[networkName];
  if (!serverUrl) {
    throw new Error(`No Arkade server URL for network: ${networkName}`);
  }

  const arkProvider: ArkProvider = new RestArkProvider(serverUrl);
  const indexerProvider: IndexerProvider = new RestIndexerProvider(serverUrl);
  const serverInfo = await arkProvider.getInfo();

  // Fetch cosigner public key
  const cosignerRes = await params.apiClient.GET("/api/delegate/cosigner-pk");
  if (!cosignerRes.data) {
    throw new Error(
      `Failed to fetch cosigner pk: ${formatApiError(cosignerRes.error)}`,
    );
  }
  const cosignerPkHex = cosignerRes.data.cosigner_pk;

  const btcNetwork = networks[networkName];
  const forfeitDecoded = Address(btcNetwork).decode(serverInfo.forfeitAddress);
  const forfeitPkScript = OutScript.encode(forfeitDecoded);

  const vhtlcPkScript = hex.encode(vhtlc.pkScript);

  const { vtxos: allVtxos } = await indexerProvider.getVtxos({
    scripts: [vhtlcPkScript],
  });
  const vtxos = allVtxos.filter((v) => !v.isSpent);

  if (vtxos.length === 0) {
    throw new Error("No settleable VTXOs found at the VHTLC address");
  }

  const totalAmount = vtxos.reduce((acc, v) => acc + BigInt(v.value), 0n);
  if (totalAmount === 0n) {
    throw new Error("Total VTXO amount is zero");
  }

  const destination = ArkAddress.decode(params.destinationAddress);

  const now = Math.floor(Date.now() / 1000);
  const intentMessage: Intent.RegisterMessage = {
    type: "register",
    onchain_output_indexes: [],
    valid_at: now,
    expire_at: now + 120,
    cosigners_public_keys: [cosignerPkHex],
  };

  const tapLeafScript = vhtlc.refund();
  const tapTree = vhtlc.encode();
  const pkScriptBytes = hex.decode(vhtlcPkScript);

  const intentInputs = vtxos.map((v) => ({
    txid: hex.decode(v.txid),
    index: v.vout,
    witnessUtxo: {
      script: pkScriptBytes,
      amount: BigInt(v.value),
    },
    tapLeafScript: [tapLeafScript],
    sighashType: SigHash.ALL,
  }));

  const intentProof = Intent.create(intentMessage, intentInputs, [
    { script: destination.pkScript, amount: totalAmount },
  ]);

  for (let i = 0; i < vtxos.length; i++) {
    setArkPsbtField(intentProof, i + 1, VtxoTaprootTree, tapTree);
  }

  const signer = SingleKey.fromHex(params.userSecretKey);
  const signedIntentProof = await signer.sign(intentProof);

  // Build forfeit PSBTs
  const dust = serverInfo.dust;
  const signedForfeitPsbts: string[] = [];

  for (const v of vtxos) {
    const vtxoAmount = BigInt(v.value);

    const forfeitTx = new Transaction({
      version: 3,
      lockTime: 0,
    });

    forfeitTx.addInput({
      txid: hex.decode(v.txid),
      index: v.vout,
      witnessUtxo: {
        script: pkScriptBytes,
        amount: vtxoAmount,
      },
      tapLeafScript: [tapLeafScript],
      sighashType: SigHash.ALL_ANYONECANPAY,
    });

    forfeitTx.addOutput({
      script: forfeitPkScript,
      amount: vtxoAmount + dust,
    });

    forfeitTx.addOutput(P2A);

    setArkPsbtField(forfeitTx, 0, VtxoTaprootTree, tapTree);

    const signedForfeit = await signer.sign(forfeitTx);
    signedForfeitPsbts.push(base64.encode(signedForfeit.toPSBT()));
  }

  // POST to lendaswap backend for receiver cosigning
  const collabRes = await params.apiClient.POST(
    "/api/swap/arkade-evm/{id}/collab-refund-delegate",
    {
      params: { path: { id: params.swapId } },
      body: {
        intent_proof: base64.encode(signedIntentProof.toPSBT()),
        forfeit_psbts: signedForfeitPsbts,
      },
    },
  );

  if (!collabRes.data) {
    throw new Error(
      `Collaborative delegate cosign failed: ${formatApiError(collabRes.error)}`,
    );
  }

  // Settle via delegate endpoint
  const settleRes = await params.apiClient.POST("/api/delegate/settle", {
    body: {
      intent_proof: collabRes.data.intent_proof,
      intent_message: Intent.encodeMessage(intentMessage),
      forfeit_psbts: collabRes.data.forfeit_psbts,
      cosigner_pk: cosignerPkHex,
      swap_id: null,
      preimage: null,
    },
  });

  if (!settleRes.data) {
    throw new Error(
      `Delegate settle failed: ${formatApiError(settleRes.error)}`,
    );
  }

  return { commitmentTxid: settleRes.data.commitment_txid };
}
