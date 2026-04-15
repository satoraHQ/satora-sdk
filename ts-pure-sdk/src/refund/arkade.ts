/**
 * Arkade (off-chain) VHTLC refund implementation.
 *
 * This module provides VHTLC refund functionality for Arkade (btc_arkade) swaps.
 * When a swap times out, users can reclaim their funds using this refund logic.
 *
 * The VHTLC uses a Taproot output with multiple spending paths. For refunds
 * after the locktime, we use the `refundWithoutReceiver` script path.
 */

import {
  type ArkProvider,
  type ArkTxInput,
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

import {
  getNetworkHrp,
  getNetworkName,
  resolveArkadeServerUrlByName,
} from "../arkade-network.js";

/** Parameters needed to build an Arkade refund */
export interface ArkadeRefundParams {
  /** User's secret key (32-byte hex) for signing */
  userSecretKey: string;
  /** User's x-only public key (32-byte hex) - this is the sender in the VHTLC */
  userPubKey: string;
  /** Lendaswap's public key (33-byte compressed hex or 32-byte x-only) - this is the receiver */
  lendaswapPubKey: string;
  /** Arkade server's x-only public key (32-byte hex) */
  arkadeServerPubKey: string;
  /** Hash lock (32-byte hex, SHA256 of preimage) - we'll compute RIPEMD160(hashLock) */
  hashLock: string;
  /** VHTLC address to refund from */
  vhtlcAddress: string;
  /** Refund locktime (unix timestamp) */
  refundLocktime: number;
  /** Unilateral claim delay in seconds */
  unilateralClaimDelay: number;
  /** Unilateral refund delay in seconds */
  unilateralRefundDelay: number;
  /** Unilateral refund without receiver delay in seconds */
  unilateralRefundWithoutReceiverDelay: number;
  /** Destination Arkade address to receive refunded funds */
  destinationAddress: string;
  /** Bitcoin network (mainnet, signet, etc.) */
  network: string;
  /** Arkade server URL (optional, uses default based on network) */
  arkadeServerUrl?: string;
}

/** Result of building an Arkade refund */
export interface ArkadeRefundResult {
  /** Virtual transaction ID */
  txId: string;
  /** Amount refunded in satoshis */
  refundAmount: bigint;
}

/**
 * Convert seconds to RelativeTimelock format.
 */
function secondsToTimelock(
  seconds: number,
): VHTLC.Options["unilateralClaimDelay"] {
  // Arkade SDK expects blocks or seconds for timelocks.
  // The sequence encoding for seconds requires 512-second granularity.
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
 * Refund a VHTLC swap after the locktime expires.
 *
 * This function:
 * 1. Constructs the VHTLC with the same parameters as the original swap
 * 2. Connects to the Arkade server
 * 3. Fetches spendable VTXOs at the VHTLC address
 * 4. Builds an offchain transaction using the refundWithoutReceiver script path
 * 5. Signs and submits the transaction
 * 6. Finalizes the transaction
 *
 * @param params - The refund parameters
 * @returns The refund result with transaction ID and amount
 * @throws Error if the refund fails
 */
export async function buildArkadeRefund(
  params: ArkadeRefundParams,
): Promise<ArkadeRefundResult> {
  const {
    userSecretKey,
    userPubKey,
    lendaswapPubKey,
    arkadeServerPubKey,
    hashLock,
    vhtlcAddress,
    refundLocktime,
    unilateralClaimDelay,
    unilateralRefundDelay,
    unilateralRefundWithoutReceiverDelay,
    destinationAddress,
    network,
    arkadeServerUrl,
  } = params;

  console.log(`Params ${JSON.stringify(params, null, 2)}`);

  // Parse keys
  const userPkBytes = parseXOnlyPubKey(userPubKey);
  const lendaswapPkBytes = parseXOnlyPubKey(lendaswapPubKey);
  const serverPkBytes = parseXOnlyPubKey(arkadeServerPubKey);

  // Compute preimage hash for VHTLC
  // The API sends us the SHA256 hash (32 bytes) or HASH160 (20 bytes)
  const hashLockBytes = hex.decode(hashLock);
  let preimageHashBytes: Uint8Array;
  if (hashLockBytes.length === 32) {
    // the preimage is a SHA256 hash, we need to compute RIPEMD160 of it
    preimageHashBytes = ripemd160(hashLockBytes);
  } else if (hashLockBytes.length === 20) {
    // Already HASH160
    preimageHashBytes = hashLockBytes;
  } else {
    throw new Error(
      `Invalid hash lock length: expected 20 or 32, got ${hashLockBytes.length}`,
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
  // For refund: user is the SENDER (refunding), lendaswap is the RECEIVER
  const vhtlc = new VHTLC.Script({
    sender: userPkBytes,
    receiver: lendaswapPkBytes,
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
  // We need the pk script to query the indexer
  const vhtlcPkScript = hex.encode(vhtlc.pkScript);

  const { vtxos } = await indexerProvider.getVtxos({
    scripts: [vhtlcPkScript],
    spendableOnly: true,
  });

  if (vtxos.length === 0) {
    const { vtxos: updated } = await indexerProvider.getVtxos({
      scripts: [vhtlcPkScript],
    });
    console.log(`Updated ${JSON.stringify(updated, null, 2)}`);
    throw new Error("No spendable VTXOs found at the VHTLC address");
  }

  // Calculate total amount (vtxos have value as number)
  const totalAmount = vtxos.reduce((acc, v) => acc + BigInt(v.value), 0n);
  if (totalAmount === 0n) {
    throw new Error("Total VTXO amount is zero");
  }

  // Get the refundWithoutReceiver TapLeafScript
  const refundLeafScript = vhtlc.refundWithoutReceiver();

  // Encode the VHTLC tap tree
  const tapTree = vhtlc.encode();

  // Decode the server's checkpoint tapscript from the server info
  const checkpointTapscript = CSVMultisigTapscript.decode(
    hex.decode(serverInfo.checkpointTapscript),
  );

  // Build inputs for offchain transaction
  const inputs: ArkTxInput[] = vtxos.map((v) => ({
    txid: v.txid,
    vout: v.vout,
    value: v.value,
    tapTree: tapTree,
    tapLeafScript: refundLeafScript,
  }));

  // Parse destination address to get the output script
  // The destination should be an Arkade address that we can decode
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

  const destinationAddressPPkScript = hex.encode(destPkScript);
  console.log(
    `Refunding ${totalAmount} to ${destinationAddress} with script ${destinationAddressPPkScript}`,
  );

  // Build the offchain transaction
  const { arkTx, checkpoints } = buildOffchainTx(
    inputs,
    outputs,
    checkpointTapscript,
  );

  // Create signer from user's secret key
  const signer = SingleKey.fromHex(userSecretKey);

  // Sign the ark transaction
  const signedArkTx = await signer.sign(arkTx);

  // Submit the transaction to the Arkade server
  const signedArkTxBase64 = base64.encode(signedArkTx.toPSBT());
  const { arkTxid, signedCheckpointTxs } = await arkProvider.submitTx(
    signedArkTxBase64,
    checkpoints.map((cp) => base64.encode(cp.toPSBT())),
  );

  const finalCheckpoints = await Promise.all(
    signedCheckpointTxs.map(async (c) => {
      const tx = Transaction.fromPSBT(base64.decode(c));
      const signedCheckpoint = await signer.sign(tx);
      return base64.encode(signedCheckpoint.toPSBT());
    }),
  );
  console.log(`Finalized tx done`);

  // Finalize the transaction
  await arkProvider.finalizeTx(arkTxid, finalCheckpoints);
  console.log(`Finalized tx submitted`, arkTxid);

  return {
    txId: arkTxid,
    refundAmount: totalAmount,
  };
}
