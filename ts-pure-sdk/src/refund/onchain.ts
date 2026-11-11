/**
 * On-chain Bitcoin HTLC refund implementation.
 *
 * This module provides Taproot HTLC refund transaction building for BTC → EVM swaps
 * where users lock on-chain Bitcoin. If the swap times out, users can reclaim
 * their funds using this refund logic.
 *
 * The HTLC uses a Taproot output with:
 * - Unspendable key spend (NUMS internal key)
 * - Hashlock script path: server claims with preimage
 * - Timelock script path: user refunds after locktime
 */

import { schnorr } from "@noble/curves/secp256k1.js";
import { ripemd160 } from "@noble/hashes/legacy.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { hex } from "@scure/base";
import * as btc from "@scure/btc-signer";

/** Bitcoin network type */
export type BitcoinNetwork = "mainnet" | "testnet" | "signet" | "regtest";

/** Parameters needed to build a claim transaction for an on-chain Taproot HTLC */
export interface OnchainClaimParams {
  /** The funding transaction ID (hex, no 0x prefix) */
  fundingTxId: string;
  /** The output index in the funding transaction */
  fundingVout: number;
  /** Amount locked in the HTLC in satoshis */
  htlcAmount: bigint;
  /** Hash lock (20-byte hex, RIPEMD160(SHA256(secret))) */
  hashLock: string;
  /** User's x-only public key (32-byte hex) — the claimer */
  userClaimPubKey: string;
  /** Server's x-only public key (32-byte hex) — the refunder */
  serverRefundPubKey: string;
  /** User's secret key (32-byte hex) for signing */
  userSecretKey: string;
  /** The preimage (32-byte hex) that hashes to the hash lock */
  preimage: string;
  /** Refund locktime (unix timestamp) — needed to reconstruct the HTLC */
  refundLocktime: number;
  /** Destination address to receive claimed funds */
  destinationAddress: string;
  /** Fee rate in satoshis per virtual byte */
  feeRateSatPerVb: number;
  /** Bitcoin network */
  network: BitcoinNetwork;
  /** Bitcoin HTLC script version. Only strict version 1 is supported. */
  btcHtlcScriptVersion?: number;
}

/** Result of building a claim transaction */
export interface OnchainClaimResult {
  /** The signed transaction hex (ready to broadcast) */
  txHex: string;
  /** Transaction ID (hash) */
  txId: string;
  /** Amount being claimed (after fees) */
  claimAmount: bigint;
  /** Fee paid in satoshis */
  fee: bigint;
  /** The HTLC address that funds are being claimed from */
  htlcAddress: string;
}

/** Parameters needed to build a refund transaction */
export interface OnchainRefundParams {
  /** The funding transaction ID (hex, no 0x prefix) */
  fundingTxId: string;
  /** The output index in the funding transaction (usually 0) */
  fundingVout: number;
  /** Amount locked in the HTLC in satoshis */
  htlcAmount: bigint;
  /** Hash lock (20-byte hex, RIPEMD160(SHA256(secret))) */
  hashLock: string;
  /** Server's x-only public key (32-byte hex) */
  serverPubKey: string;
  /** User's x-only public key (32-byte hex) */
  userPubKey: string;
  /** User's secret key (32-byte hex) for signing */
  userSecretKey: string;
  /** Refund locktime (unix timestamp) */
  refundLocktime: number;
  /** Destination address to receive refunded funds */
  destinationAddress: string;
  /** Fee rate in satoshis per virtual byte */
  feeRateSatPerVb: number;
  /** Bitcoin network */
  network: BitcoinNetwork;
  /** Bitcoin HTLC script version. Only strict version 1 is supported. */
  btcHtlcScriptVersion?: number;
}

/** Result of building a refund transaction */
export interface OnchainRefundResult {
  /** The signed transaction hex (ready to broadcast) */
  txHex: string;
  /** Transaction ID (hash) */
  txId: string;
  /** Amount being refunded (after fees) */
  refundAmount: bigint;
  /** Fee paid in satoshis */
  fee: bigint;
  /** The HTLC address that funds are being refunded from */
  htlcAddress: string;
}

/**
 * NUMS (Nothing Up My Sleeve) point - provably unspendable public key.
 * This is the standard BIP-341 NUMS point used as the internal key
 * for script-only Taproot outputs.
 */
const NUMS_POINT = hex.decode(
  "0250929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0",
);

/**
 * Regtest network configuration.
 * btc-signer only provides NETWORK (mainnet) and TEST_NETWORK (testnet/signet),
 * but regtest uses a different bech32 prefix ('bcrt' instead of 'tb').
 */
const REGTEST_NETWORK = {
  bech32: "bcrt",
  pubKeyHash: 0x6f,
  scriptHash: 0xc4,
  wif: 0xef,
} as const;

/**
 * Get the btc-signer network configuration.
 */
function getNetwork(
  network: BitcoinNetwork,
): typeof btc.NETWORK | typeof REGTEST_NETWORK {
  switch (network) {
    case "mainnet":
      return btc.NETWORK;
    case "testnet":
    case "signet":
      return btc.TEST_NETWORK;
    case "regtest":
      return REGTEST_NETWORK;
    default:
      throw new Error(`Unknown network: ${network}`);
  }
}

/**
 * Build the hashlock tapscript for server claim.
 *
 * Script: `<server_pk> OP_CHECKSIGVERIFY OP_SIZE 32 OP_EQUALVERIFY OP_HASH160 <hash_lock> OP_EQUAL`
 *
 * The server must provide a valid Schnorr signature AND a 32-byte preimage.
 */
function buildHashlockScript(
  hashLock: Uint8Array,
  serverPubKey: Uint8Array,
  btcHtlcScriptVersion = 1,
): Uint8Array {
  if (btcHtlcScriptVersion !== 1) {
    throw new Error(
      `Unsupported BTC HTLC script version: ${btcHtlcScriptVersion}`,
    );
  }
  const script = [
    serverPubKey,
    "CHECKSIGVERIFY",
    "SIZE",
    32,
    "EQUALVERIFY",
    "HASH160",
    hashLock,
    "EQUAL",
  ] as Parameters<typeof btc.Script.encode>[0];
  return btc.Script.encode(script);
}

/**
 * Build the timelock tapscript for user refund.
 *
 * Script: `<locktime> OP_CLTV OP_DROP <user_pk> OP_CHECKSIG`
 *
 * The user can spend after the locktime has passed.
 */
function buildTimelockScript(
  userPubKey: Uint8Array,
  refundLocktime: number,
): Uint8Array {
  return btc.Script.encode([
    refundLocktime,
    "CHECKLOCKTIMEVERIFY",
    "DROP",
    userPubKey,
    "CHECKSIG",
  ]);
}

/**
 * Compute HASH160 (RIPEMD160(SHA256(data))).
 */
export function computeHash160(data: Uint8Array): Uint8Array {
  return ripemd160(sha256(data));
}

/**
 * Build a Taproot HTLC spending info.
 *
 * Creates the P2TR structure with:
 * - Unspendable internal key (NUMS point)
 * - Left leaf: hashlock script (server claim)
 * - Right leaf: timelock script (user refund)
 */
function buildHtlcTaprootInfo(
  hashLock: Uint8Array,
  serverPubKey: Uint8Array,
  userPubKey: Uint8Array,
  refundLocktime: number,
  btcHtlcScriptVersion = 1,
): {
  hashlockScript: Uint8Array;
  timelockScript: Uint8Array;
  p2tr: ReturnType<typeof btc.p2tr>;
} {
  const hashlockScript = buildHashlockScript(
    hashLock,
    serverPubKey,
    btcHtlcScriptVersion,
  );
  const timelockScript = buildTimelockScript(userPubKey, refundLocktime);

  // Build the taproot tree with two leaves
  // Using NUMS point as internal key (script-path only)
  const p2tr = btc.p2tr(
    NUMS_POINT.slice(1), // Remove the 02/03 prefix for x-only
    [{ script: hashlockScript }, { script: timelockScript }],
    undefined,
    true, // allowUnknownOutputs
  );

  return { hashlockScript, timelockScript, p2tr };
}

/**
 * Estimate the virtual size of a Taproot script-path refund transaction.
 *
 * Components:
 * - Version: 4 bytes
 * - Marker + Flag: 2 bytes (for witness)
 * - Input count: 1 byte
 * - Input: 32 (txid) + 4 (vout) + 1 (script len) + 4 (sequence) = 41 bytes
 * - Output count: 1 byte
 * - Output: 8 (value) + 1 (script len) + 34 (P2TR output) = 43 bytes
 * - Witness: ~130 bytes (sig + script + control block)
 * - Locktime: 4 bytes
 *
 * Total base: ~96 bytes, witness: ~130 bytes
 * vBytes = base + witness/4 ≈ 96 + 33 = 129 vBytes
 *
 * Using a conservative estimate of 130 vBytes.
 */
const REFUND_TX_VBYTES = 130n;

/**
 * Estimate vBytes for a claim transaction.
 * Similar to refund but includes the preimage in the witness (~32 bytes extra).
 * Conservative estimate: 150 vBytes.
 */
const CLAIM_TX_VBYTES = 150n;

/**
 * Build and sign a claim transaction for an on-chain Bitcoin Taproot HTLC.
 *
 * This creates a transaction that spends from the HTLC using the hashlock
 * script path by providing the preimage and a valid signature.
 *
 * Used for EVM-to-Bitcoin swaps where the user claims BTC after the server
 * has funded the on-chain HTLC.
 *
 * @param params - The claim parameters
 * @returns The signed transaction and related info
 * @throws Error if the transaction cannot be built
 */
export function buildOnchainClaimTransaction(
  params: OnchainClaimParams,
): OnchainClaimResult {
  const {
    fundingTxId,
    fundingVout,
    htlcAmount,
    hashLock,
    userClaimPubKey,
    serverRefundPubKey,
    userSecretKey,
    preimage,
    refundLocktime,
    destinationAddress,
    feeRateSatPerVb,
    network,
  } = params;

  // Parse hex inputs
  const hashLockBytes = hex.decode(hashLock);
  const userClaimPkBytes = hex.decode(userClaimPubKey);
  const serverRefundPkBytes = hex.decode(serverRefundPubKey);
  const userSkBytes = hex.decode(userSecretKey);
  const preimageBytes = hex.decode(preimage);

  if (hashLockBytes.length !== 20) {
    throw new Error(
      `Invalid hash lock length: expected 20, got ${hashLockBytes.length}`,
    );
  }
  if (userClaimPkBytes.length !== 32) {
    throw new Error(
      `Invalid user claim pubkey length: expected 32, got ${userClaimPkBytes.length}`,
    );
  }
  if (serverRefundPkBytes.length !== 32) {
    throw new Error(
      `Invalid server refund pubkey length: expected 32, got ${serverRefundPkBytes.length}`,
    );
  }
  if (userSkBytes.length !== 32) {
    throw new Error(
      `Invalid user secret key length: expected 32, got ${userSkBytes.length}`,
    );
  }
  if (preimageBytes.length !== 32) {
    throw new Error(
      `Invalid preimage length: expected 32, got ${preimageBytes.length}`,
    );
  }

  // Verify preimage matches hash lock
  const computedHashLock = computeHash160(preimageBytes);
  if (hex.encode(computedHashLock) !== hex.encode(hashLockBytes)) {
    throw new Error(
      "Preimage does not match hash lock: HASH160(preimage) != hashLock",
    );
  }

  // Build the HTLC Taproot structure
  // For evm_to_bitcoin: user is claimer (hashlock), server is refunder (timelock)
  // buildHtlcTaprootInfo(hashLock, claimerPk, refunderPk, locktime)
  const { p2tr } = buildHtlcTaprootInfo(
    hashLockBytes,
    userClaimPkBytes,
    serverRefundPkBytes,
    refundLocktime,
    params.btcHtlcScriptVersion,
  );

  // Calculate fee
  const fee = CLAIM_TX_VBYTES * BigInt(Math.ceil(feeRateSatPerVb));
  if (fee >= htlcAmount) {
    throw new Error(
      `Fee (${fee} sats) exceeds HTLC amount (${htlcAmount} sats)`,
    );
  }
  const claimAmount = htlcAmount - fee;

  // Get network config
  const networkConfig = getNetwork(network);

  // Find the tapLeafScript for the hashlock script (index 0 in our tree)
  const tapLeafScript = p2tr.tapLeafScript;
  if (!tapLeafScript || tapLeafScript.length < 1) {
    throw new Error("Failed to build tapLeafScript for hashlock");
  }

  // The hashlock script is at index 0 in our tree
  const hashlockLeaf = tapLeafScript[0];

  // Build the transaction
  const tx = new btc.Transaction({
    allowUnknownOutputs: true,
    allowUnknownInputs: true,
  });

  // Add input (the HTLC output we're spending)
  tx.addInput({
    txid: fundingTxId,
    index: fundingVout,
    witnessUtxo: {
      script: p2tr.script,
      amount: htlcAmount,
    },
    tapLeafScript: [hashlockLeaf],
    sequence: 0xffffffff,
  });

  // Add output (destination)
  tx.addOutputAddress(destinationAddress, claimAmount, networkConfig);

  // Manually compute sighash, sign, and build the full witness.
  // We can't use tx.signIdx() because finalize() only produces [signature]
  // but the hashlock script requires [preimage, signature] in the witness.
  // After signIdx, btc-signer locks the transaction and won't let us modify
  // the witness fields. So we do everything manually.

  const [controlBlockInfo, leafScriptWithVersion] = hashlockLeaf;

  // tapLeafScript entries have the leaf version byte appended to the script.
  // Strip it for sighash computation and the witness.
  const leafScript = leafScriptWithVersion.slice(0, -1);
  const leafVersion = leafScriptWithVersion[leafScriptWithVersion.length - 1];

  // Compute the Taproot script-path sighash (BIP 342)
  // preimageWitnessV1 returns the final tagged hash, not a preimage to be hashed again.
  const sighash = tx.preimageWitnessV1(
    0,
    [p2tr.script],
    btc.SigHash.DEFAULT,
    [htlcAmount],
    undefined,
    leafScript,
    leafVersion,
  );

  // Sign the sighash with Schnorr (BIP 340)
  const sig = schnorr.sign(sighash, userSkBytes);

  // Encode the control block struct to raw bytes:
  // [version(1)] [internalKey(32)] [merklePath(32*n)]
  const cbInfo = controlBlockInfo as {
    version: number;
    internalKey: Uint8Array;
    merklePath: Uint8Array[];
  };
  const encodedControlBlock = new Uint8Array(
    1 + 32 + 32 * cbInfo.merklePath.length,
  );
  encodedControlBlock[0] = cbInfo.version;
  encodedControlBlock.set(cbInfo.internalKey, 1);
  for (let i = 0; i < cbInfo.merklePath.length; i++) {
    encodedControlBlock.set(cbInfo.merklePath[i], 33 + 32 * i);
  }

  // Set the final witness directly (no signIdx needed):
  // [preimage, signature, script, control_block]
  tx.updateInput(0, {
    finalScriptWitness: [preimageBytes, sig, leafScript, encodedControlBlock],
  });

  // Extract the signed transaction
  const txHex = hex.encode(tx.extract());
  const txId = tx.id;

  // Compute the HTLC address
  const htlcAddress = btc.Address(networkConfig).encode({
    type: "tr",
    pubkey: p2tr.tweakedPubkey,
  });

  return {
    txHex,
    txId,
    claimAmount,
    fee,
    htlcAddress,
  };
}

/**
 * Build and sign a refund transaction for an on-chain Bitcoin HTLC.
 *
 * This creates a transaction that spends from the HTLC using the timelock
 * script path after the refund locktime has passed.
 *
 * @param params - The refund parameters
 * @returns The signed transaction and related info
 * @throws Error if the transaction cannot be built
 */
export function buildOnchainRefundTransaction(
  params: OnchainRefundParams,
): OnchainRefundResult {
  const {
    fundingTxId,
    fundingVout,
    htlcAmount,
    hashLock,
    serverPubKey,
    userPubKey,
    userSecretKey,
    refundLocktime,
    destinationAddress,
    feeRateSatPerVb,
    network,
  } = params;

  // Parse hex inputs
  const hashLockBytes = hex.decode(hashLock);
  const serverPkBytes = hex.decode(serverPubKey);
  const userPkBytes = hex.decode(userPubKey);
  const userSkBytes = hex.decode(userSecretKey);

  if (hashLockBytes.length !== 20) {
    throw new Error(
      `Invalid hash lock length: expected 20, got ${hashLockBytes.length}`,
    );
  }
  if (serverPkBytes.length !== 32) {
    throw new Error(
      `Invalid server pubkey length: expected 32, got ${serverPkBytes.length}`,
    );
  }
  if (userPkBytes.length !== 32) {
    throw new Error(
      `Invalid user pubkey length: expected 32, got ${userPkBytes.length}`,
    );
  }
  if (userSkBytes.length !== 32) {
    throw new Error(
      `Invalid user secret key length: expected 32, got ${userSkBytes.length}`,
    );
  }

  // Build the HTLC Taproot structure
  const { p2tr } = buildHtlcTaprootInfo(
    hashLockBytes,
    serverPkBytes,
    userPkBytes,
    refundLocktime,
    params.btcHtlcScriptVersion,
  );

  // Calculate fee
  const fee = REFUND_TX_VBYTES * BigInt(Math.ceil(feeRateSatPerVb));
  if (fee >= htlcAmount) {
    throw new Error(
      `Fee (${fee} sats) exceeds HTLC amount (${htlcAmount} sats)`,
    );
  }
  const refundAmount = htlcAmount - fee;

  // Get network config
  const networkConfig = getNetwork(network);

  // Find the tapLeafScript for the timelock script (index 1 in our tree)
  // p2tr.tapLeafScript is an array of [controlBlockInfo, script] tuples
  const tapLeafScript = p2tr.tapLeafScript;
  if (!tapLeafScript || tapLeafScript.length < 2) {
    throw new Error("Failed to build tapLeafScript for timelock");
  }

  // The timelock script is at index 1 in our tree
  const timelockLeaf = tapLeafScript[1];

  // Build the transaction with lockTime in constructor
  const tx = new btc.Transaction({
    allowUnknownOutputs: true,
    allowUnknownInputs: true,
    lockTime: refundLocktime,
  });

  // Add input (the HTLC output we're spending)
  tx.addInput({
    txid: fundingTxId,
    index: fundingVout,
    witnessUtxo: {
      script: p2tr.script,
      amount: htlcAmount,
    },
    tapLeafScript: [timelockLeaf],
    sequence: 0xfffffffe, // Enable locktime (< 0xffffffff)
  });

  // Add output (destination)
  tx.addOutputAddress(destinationAddress, refundAmount, networkConfig);

  // Sign the input
  tx.signIdx(userSkBytes, 0);

  // Finalize
  tx.finalize();

  // Extract the signed transaction
  const txHex = hex.encode(tx.extract());
  const txId = tx.id;

  // Compute the HTLC address
  const htlcAddress = btc.Address(networkConfig).encode({
    type: "tr",
    pubkey: p2tr.tweakedPubkey,
  });

  return {
    txHex,
    txId,
    refundAmount,
    fee,
    htlcAddress,
  };
}

/**
 * Verify that a Taproot address matches the expected HTLC parameters.
 *
 * This is useful to confirm the HTLC address returned by the server
 * matches what we expect based on the swap parameters.
 *
 * @param expectedAddress - The address to verify
 * @param hashLock - Hash lock (20-byte hex)
 * @param serverPubKey - Server's x-only public key (32-byte hex)
 * @param userPubKey - User's x-only public key (32-byte hex)
 * @param refundLocktime - Refund locktime (unix timestamp)
 * @param network - Bitcoin network
 * @param btcHtlcScriptVersion - Bitcoin HTLC script version. Only strict version 1 is supported.
 * @returns true if the address matches, false otherwise
 */
export function verifyHtlcAddress(
  expectedAddress: string,
  hashLock: string,
  serverPubKey: string,
  userPubKey: string,
  refundLocktime: number,
  network: BitcoinNetwork,
  btcHtlcScriptVersion = 1,
): boolean {
  return (
    htlcAddress(
      hashLock,
      serverPubKey,
      userPubKey,
      refundLocktime,
      network,
      btcHtlcScriptVersion,
    ) === expectedAddress
  );
}

/**
 * The Taproot address of the HTLC on these terms, with the same arguments as
 * {@link verifyHtlcAddress}.
 */
export function htlcAddress(
  hashLock: string,
  serverPubKey: string,
  userPubKey: string,
  refundLocktime: number,
  network: BitcoinNetwork,
  btcHtlcScriptVersion = 1,
): string {
  const hashLockBytes = hex.decode(hashLock);
  const serverPkBytes = hex.decode(serverPubKey);
  const userPkBytes = hex.decode(userPubKey);

  const { p2tr } = buildHtlcTaprootInfo(
    hashLockBytes,
    serverPkBytes,
    userPkBytes,
    refundLocktime,
    btcHtlcScriptVersion,
  );

  return btc.Address(getNetwork(network)).encode({
    type: "tr",
    pubkey: p2tr.tweakedPubkey,
  });
}
