/**
 * Arkade (off-chain) VHTLC claim implementation.
 *
 * This module provides VHTLC claim functionality for Arkade swaps.
 * When a swap is funded by the server (EVM-to-Arkade direction),
 * the user can claim their BTC by revealing the preimage.
 *
 * The VHTLC uses a Taproot output with multiple spending paths. For claims,
 * we use the `claim` script path which requires revealing the preimage.
 */

import {
  type ArkProvider,
  type ArkTxInput,
  buildOffchainTx,
  ConditionWitness,
  CSVMultisigTapscript,
  type IndexerProvider,
  Intent,
  RestArkProvider,
  RestIndexerProvider,
  type SignedIntent,
  SingleKey,
  setArkPsbtField,
  Transaction,
  VHTLC,
  VtxoTaprootTree,
} from "@arkade-os/sdk";
import { ripemd160 } from "@noble/hashes/legacy.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { base64, hex } from "@scure/base";
import { SigHash } from "@scure/btc-signer";

import {
  getNetworkHrp,
  getNetworkName,
  resolveArkadeServerUrlByName,
} from "../arkade-network.js";

/** Parameters needed to build an Arkade claim */
export interface ArkadeClaimParams {
  /** User's secret key (32-byte hex) for signing */
  userSecretKey: string;
  /** User's x-only public key (32-byte hex) - this is the RECEIVER in the VHTLC */
  userPubKey: string;
  /** Lendaswap's public key (33-byte compressed hex or 32-byte x-only) - this is the SENDER */
  lendaswapPubKey: string;
  /** Arkade server's x-only public key (32-byte hex) */
  arkadeServerPubKey: string;
  /** Preimage (32-byte hex) - reveals the secret to claim */
  preimage: string;
  /** Preimage Hash */
  preimageHash: string;
  /** VHTLC address to claim from */
  vhtlcAddress: string;
  /** Refund locktime (unix timestamp) */
  refundLocktime: number;
  /** Unilateral claim delay in seconds */
  unilateralClaimDelay: number;
  /** Unilateral refund delay in seconds */
  unilateralRefundDelay: number;
  /** Unilateral refund without receiver delay in seconds */
  unilateralRefundWithoutReceiverDelay: number;
  /** Destination Arkade address to receive claimed funds */
  destinationAddress: string;
  /** Bitcoin network (mainnet, signet, etc.) */
  network: string;
  /** Arkade server URL (optional, uses default based on network) */
  arkadeServerUrl?: string;
}

/** Result of building an Arkade claim */
export interface ArkadeClaimResult {
  /** Virtual transaction ID */
  txId: string;
  /** Amount claimed in satoshis */
  claimAmount: bigint;
}

/**
 * Convert seconds to RelativeTimelock format.
 */
function secondsToTimelock(
  seconds: number,
): VHTLC.Options["unilateralClaimDelay"] {
  return {
    type: "seconds" as const,
    value: BigInt(seconds),
  };
}

/**
 * Parse public key, handling both compressed (33-byte) and x-only (32-byte) formats.
 * Returns x-only (32-byte) format.
 */
function parseXOnlyPubKey(pubKeyHex: string): Uint8Array {
  const bytes = hex.decode(pubKeyHex);
  if (bytes.length === 33) {
    // Compressed pubkey, strip the prefix byte
    return bytes.slice(1);
  }
  if (bytes.length === 32) {
    // Already x-only
    return bytes;
  }
  throw new Error(
    `Invalid public key length: expected 32 or 33, got ${bytes.length}`,
  );
}

/**
 * Claim a VHTLC swap by revealing the preimage.
 *
 * This function:
 * 1. Constructs the VHTLC with the same parameters as the original swap
 * 2. Connects to the Arkade server
 * 3. Fetches spendable VTXOs at the VHTLC address
 * 4. Builds an offchain transaction using the claim script path
 * 5. Signs and submits the transaction (with preimage in witness)
 * 6. Finalizes the transaction
 *
 * @param params - The claim parameters
 * @returns The claim result with transaction ID and amount
 * @throws Error if the claim fails
 */
export async function buildArkadeClaim(
  params: ArkadeClaimParams,
): Promise<ArkadeClaimResult> {
  const {
    userSecretKey,
    userPubKey,
    lendaswapPubKey,
    arkadeServerPubKey,
    preimage,
    preimageHash,
    vhtlcAddress,
    refundLocktime,
    unilateralClaimDelay,
    unilateralRefundDelay,
    unilateralRefundWithoutReceiverDelay,
    destinationAddress,
    network,
    arkadeServerUrl,
  } = params;

  console.log(`Arkade claim params: ${JSON.stringify(params, null, 2)}`);

  // Parse keys
  // For claim: user is RECEIVER, lendaswap is SENDER
  const userPkBytes = parseXOnlyPubKey(userPubKey);
  const lendaswapPkBytes = parseXOnlyPubKey(lendaswapPubKey);
  const serverPkBytes = parseXOnlyPubKey(arkadeServerPubKey);

  // Parse preimage and compute hash
  const preimageBytes = hex.decode(preimage);
  if (preimageBytes.length !== 32) {
    throw new Error(
      `Invalid preimage length: expected 32, got ${preimageBytes.length}`,
    );
  }

  // Compute preimage hash: SHA256 -> RIPEMD160 (HASH160)
  const sha256Hash = sha256(preimageBytes);
  const preimageHashBytes = ripemd160(sha256Hash);

  const preimageHashBytesString = hex.encode(preimageHashBytes);
  if (
    preimageHashBytesString !== hex.encode(ripemd160(hex.decode(preimageHash)))
  ) {
    throw new Error(
      `Preimage hash are not equal. '${hex.encode(ripemd160(hex.decode(preimageHash)))}' vs ${preimageHashBytesString}'`,
    );
  }

  // Determine Arkade server URL
  const networkName = getNetworkName(network);
  const serverUrl = resolveArkadeServerUrlByName(networkName, arkadeServerUrl);

  // Create Arkade providers
  const arkProvider: ArkProvider = new RestArkProvider(serverUrl);
  const indexerProvider: IndexerProvider = new RestIndexerProvider(serverUrl);

  // Get server info
  const serverInfo = await arkProvider.getInfo();

  // Construct VHTLC with the same parameters as the original swap
  // For claim: lendaswap is the SENDER, user is the RECEIVER
  const vhtlc = new VHTLC.Script({
    sender: lendaswapPkBytes,
    receiver: userPkBytes,
    server: serverPkBytes,
    preimageHash: preimageHashBytes,
    refundLocktime: BigInt(refundLocktime),
    unilateralClaimDelay: secondsToTimelock(unilateralClaimDelay),
    unilateralRefundDelay: secondsToTimelock(unilateralRefundDelay),
    unilateralRefundWithoutReceiverDelay: secondsToTimelock(
      unilateralRefundWithoutReceiverDelay,
    ),
  });

  // Get network HRP and verify computed VHTLC address
  const hrp = getNetworkHrp(networkName);
  const computedAddress = vhtlc.address(hrp, serverPkBytes);
  const computedAddressStr = computedAddress.encode();

  // Verify address matches expected
  if (computedAddressStr !== vhtlcAddress) {
    throw new Error(
      `Computed VHTLC address (${computedAddressStr}) does not match expected (${vhtlcAddress})`,
    );
  }

  // Fetch VTXOs at the VHTLC address
  const vhtlcPkScript = hex.encode(vhtlc.pkScript);

  const { vtxos } = await indexerProvider.getVtxos({
    scripts: [vhtlcPkScript],
    spendableOnly: true,
  });

  if (vtxos.length === 0) {
    const { vtxos: allVtxos } = await indexerProvider.getVtxos({
      scripts: [vhtlcPkScript],
    });
    console.log(`All VTXOs at address: ${JSON.stringify(allVtxos, null, 2)}`);
    throw new Error("No spendable VTXOs found at the VHTLC address");
  }

  // Calculate total amount
  const totalAmount = vtxos.reduce((acc, v) => acc + BigInt(v.value), 0n);
  if (totalAmount === 0n) {
    throw new Error("Total VTXO amount is zero");
  }

  // Get the claim TapLeafScript (this is the key difference from refund)
  const claimLeafScript = vhtlc.claim();

  // Encode the VHTLC tap tree
  const tapTree = vhtlc.encode();

  // Decode the server's checkpoint tapscript
  const checkpointTapscript = CSVMultisigTapscript.decode(
    hex.decode(serverInfo.checkpointTapscript),
  );

  // Build inputs for offchain transaction
  // Include the preimage in the witness data
  const inputs: ArkTxInput[] = vtxos.map((v) => ({
    txid: v.txid,
    vout: v.vout,
    value: v.value,
    tapTree: tapTree,
    tapLeafScript: claimLeafScript,
    // The preimage will be added to the witness during signing
    witnessData: preimageBytes,
  }));

  // Parse destination address
  const { ArkAddress } = await import("@arkade-os/sdk");
  const destAddr = ArkAddress.decode(destinationAddress);
  const destPkScript = destAddr.pkScript;

  // Build outputs
  const outputs = [
    {
      script: destPkScript,
      amount: totalAmount,
    },
  ];

  console.log(`Claiming ${totalAmount} sats to ${destinationAddress}`);

  // Build the offchain transaction
  const { arkTx, checkpoints } = buildOffchainTx(
    inputs,
    outputs,
    checkpointTapscript,
  );

  // Create signer from user's secret key
  const signer = SingleKey.fromHex(userSecretKey);
  const computedPk = await signer.xOnlyPublicKey();
  if (hex.encode(userPkBytes) !== hex.encode(computedPk)) {
    throw new Error(
      `Signing with wrong key? ${hex.encode(userPkBytes)} vs ${hex.encode(computedPk)}`,
    );
  }

  // Sign the ark transaction
  setArkPsbtField(arkTx, 0, ConditionWitness, [preimageBytes]);
  const signedArkTx = await signer.sign(arkTx);

  // Submit the transaction to the Arkade server
  const signedArkTxBase64 = base64.encode(signedArkTx.toPSBT());
  const { arkTxid, signedCheckpointTxs } = await arkProvider.submitTx(
    signedArkTxBase64,
    checkpoints.map((cp) => base64.encode(cp.toPSBT())),
  );

  // Sign and finalize checkpoint transactions
  const finalCheckpoints = await Promise.all(
    signedCheckpointTxs.map(async (c) => {
      const tx = Transaction.fromPSBT(base64.decode(c));
      setArkPsbtField(tx, 0, ConditionWitness, [preimageBytes]);
      const signedCheckpoint = await signer.sign(tx, [0]);
      return base64.encode(signedCheckpoint.toPSBT());
    }),
  );

  console.log(`Checkpoint transactions signed`);

  // Finalize the transaction
  try {
    await arkProvider.finalizeTx(arkTxid, finalCheckpoints);
  } catch (error) {
    console.error(
      `Failed claiming funds. Please scream loudly, cry, and ask for help.`,
      error,
    );
    throw error;
  }
  console.log(`Arkade claim finalized: ${arkTxid}`);

  return {
    txId: arkTxid,
    claimAmount: totalAmount,
  };
}

export async function continueArkadeClaim(
  params: ArkadeClaimParams,
): Promise<ArkadeClaimResult> {
  const {
    userSecretKey,
    userPubKey,
    lendaswapPubKey,
    arkadeServerPubKey,
    preimage,
    preimageHash,
    vhtlcAddress,
    refundLocktime,
    unilateralClaimDelay,
    unilateralRefundDelay,
    unilateralRefundWithoutReceiverDelay,
    // destinationAddress not needed — we continue an existing pending tx
    network,
    arkadeServerUrl,
  } = params;

  console.log(
    `Continuing Arkade claim with params: ${JSON.stringify(params, null, 2)}`,
  );

  // Parse keys
  // For claim: user is RECEIVER, lendaswap is SENDER
  const userPkBytes = parseXOnlyPubKey(userPubKey);
  const lendaswapPkBytes = parseXOnlyPubKey(lendaswapPubKey);
  const serverPkBytes = parseXOnlyPubKey(arkadeServerPubKey);

  // Parse preimage and compute hash
  const preimageBytes = hex.decode(preimage);
  if (preimageBytes.length !== 32) {
    throw new Error(
      `Invalid preimage length: expected 32, got ${preimageBytes.length}`,
    );
  }

  // Compute preimage hash: SHA256 -> RIPEMD160 (HASH160)
  const sha256Hash = sha256(preimageBytes);
  const preimageHashBytes = ripemd160(sha256Hash);

  const preimageHashBytesString = hex.encode(preimageHashBytes);
  if (
    preimageHashBytesString !== hex.encode(ripemd160(hex.decode(preimageHash)))
  ) {
    throw new Error(
      `Preimage hash are not equal. '${hex.encode(ripemd160(hex.decode(preimageHash)))}' vs ${preimageHashBytesString}'`,
    );
  }

  // Determine Arkade server URL
  const networkName = getNetworkName(network);
  const serverUrl = resolveArkadeServerUrlByName(networkName, arkadeServerUrl);

  // Create Arkade providers
  const arkProvider: ArkProvider = new RestArkProvider(serverUrl);
  const indexerProvider: IndexerProvider = new RestIndexerProvider(serverUrl);

  // Construct VHTLC with the same parameters as the original swap
  // For claim: lendaswap is the SENDER, user is the RECEIVER
  const vhtlc = new VHTLC.Script({
    sender: lendaswapPkBytes,
    receiver: userPkBytes,
    server: serverPkBytes,
    preimageHash: preimageHashBytes,
    refundLocktime: BigInt(refundLocktime),
    unilateralClaimDelay: secondsToTimelock(unilateralClaimDelay),
    unilateralRefundDelay: secondsToTimelock(unilateralRefundDelay),
    unilateralRefundWithoutReceiverDelay: secondsToTimelock(
      unilateralRefundWithoutReceiverDelay,
    ),
  });

  // Get network HRP and verify computed VHTLC address
  const hrp = getNetworkHrp(networkName);
  const computedAddress = vhtlc.address(hrp, serverPkBytes);
  const computedAddressStr = computedAddress.encode();

  // Verify address matches expected
  if (computedAddressStr !== vhtlcAddress) {
    throw new Error(
      `Computed VHTLC address (${computedAddressStr}) does not match expected (${vhtlcAddress})`,
    );
  }

  // Fetch VTXOs at the VHTLC address
  const vhtlcPkScript = hex.encode(vhtlc.pkScript);

  const { vtxos } = await indexerProvider.getVtxos({
    scripts: [vhtlcPkScript],
  });

  if (vtxos.length === 0) {
    const { vtxos: allVtxos } = await indexerProvider.getVtxos({
      scripts: [vhtlcPkScript],
    });
    console.log(`All VTXOs at address: ${JSON.stringify(allVtxos, null, 2)}`);
    throw new Error("No spendable VTXOs found at the VHTLC address");
  }

  // Calculate total amount
  const totalAmount = vtxos.reduce((acc, v) => acc + BigInt(v.value), 0n);
  if (totalAmount === 0n) {
    throw new Error("Total VTXO amount is zero");
  }

  // Get the claim TapLeafScript (needed for the GetPendingTx intent)
  const claimLeafScript = vhtlc.claim();
  const claimScriptByte = claimLeafScript[1];

  // Encode the VHTLC tap tree
  const tapTree = vhtlc.encode();

  console.log(`Continuing claim of ${totalAmount} sats from ${vhtlcAddress}`);

  // Build a GetPendingTx intent to ask Arkade for pending transactions
  const now = Math.floor(Date.now() / 1000);
  const intentMessage: Intent.GetPendingTxMessage = {
    type: "get-pending-tx",
    expire_at: now + 120,
  };

  // Build intent inputs from VTXOs
  const pkScriptBytes = hex.decode(vhtlcPkScript);
  const intentInputs = vtxos.map((v) => ({
    txid: hex.decode(v.txid),
    index: v.vout,
    witnessUtxo: {
      script: pkScriptBytes,
      amount: BigInt(v.value),
    },
    tapLeafScript: [claimLeafScript],
    sighashType: SigHash.ALL,
  }));

  // Create the intent proof PSBT
  const intentProof = Intent.create(intentMessage, intentInputs);

  // Set VtxoTaprootTree on each real input (skip input 0 which is the toSpend ref)
  for (let i = 0; i < vtxos.length; i++) {
    setArkPsbtField(intentProof, i + 1, VtxoTaprootTree, tapTree);
  }

  // Set condition witness (preimage) on each real input
  for (let i = 0; i < vtxos.length; i++) {
    setArkPsbtField(intentProof, i + 1, ConditionWitness, [preimageBytes]);
  }

  // Create signer from user's secret key
  const signer = SingleKey.fromHex(userSecretKey);
  const computedPk = await signer.xOnlyPublicKey();
  if (hex.encode(userPkBytes) !== hex.encode(computedPk)) {
    throw new Error(
      `Signing with wrong key? ${hex.encode(userPkBytes)} vs ${hex.encode(computedPk)}`,
    );
  }

  // Sign the intent proof
  const signedIntentProof = await signer.sign(intentProof);
  const signedIntent: SignedIntent<Intent.GetPendingTxMessage> = {
    proof: base64.encode(signedIntentProof.toPSBT()),
    message: intentMessage,
  };

  // Fetch pending transactions from Arkade
  const pendingTxs = await arkProvider.getPendingTxs(signedIntent);

  if (pendingTxs.length === 0) {
    throw new Error(
      "No pending transactions found at the VHTLC address. The claim may have already been finalized or was never submitted.",
    );
  }

  console.log(`Found ${pendingTxs.length} pending transaction(s)`);

  // Finalize each pending transaction
  let lastResult: ArkadeClaimResult | undefined;

  for (const pendingTx of pendingTxs) {
    const { arkTxid, signedCheckpointTxs } = pendingTx;

    // Sign and finalize checkpoint transactions
    const finalCheckpoints = await Promise.all(
      signedCheckpointTxs.map(async (c) => {
        const checkpointTx = Transaction.fromPSBT(base64.decode(c));

        // Restore missing witness scripts from the ark tx
        checkpointTx.updateInput(0, {
          witnessScript: claimScriptByte,
        });

        // Inject preimage into all checkpoint inputs
        for (let i = 0; i < checkpointTx.inputsLength; i++) {
          setArkPsbtField(checkpointTx, i, ConditionWitness, [preimageBytes]);
        }

        // Sign input 0 (the checkpoint input)
        const signedCheckpoint = await signer.sign(checkpointTx, [0]);
        return base64.encode(signedCheckpoint.toPSBT());
      }),
    );

    // Finalize the transaction

    try {
      await arkProvider.finalizeTx(arkTxid, finalCheckpoints);
    } catch (error) {
      console.error(
        `Failed continuing claiming funds. Please scream loudly, cry, and ask for help.`,
        error,
      );
      throw error;
    }

    console.log(`Arkade claim finalized: ${arkTxid}`);

    lastResult = {
      txId: arkTxid,
      claimAmount: totalAmount,
    };
  }

  if (!lastResult) {
    throw Error("Failed continuing claim");
  }
  // We know lastResult is defined because we checked pendingTxs.length > 0
  return lastResult;
}
