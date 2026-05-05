/**
 * Delegate settlement for VHTLC VTXOs.
 *
 * Allows the client to prepare and sign delegate PSBTs (intent + forfeits),
 * then POST them to the lendaswap backend which runs the Ark batch ceremony.
 *
 * This works for spendable, recoverable, AND expired VTXOs — unlike the
 * offchain submitTx/finalizeTx path which only handles spendable VTXOs.
 */

import {
  ArkAddress,
  type ArkProvider,
  ConditionWitness,
  type IndexerProvider,
  Intent,
  type NetworkName,
  networks,
  RestArkProvider,
  RestIndexerProvider,
  SingleKey,
  setArkPsbtField,
  type TapLeafScript,
  Transaction,
  VHTLC,
  VtxoTaprootTree,
} from "@arkade-os/sdk";
import { Address, OutScript, SigHash } from "@scure/btc-signer";

// P2A is the zero-value anchor output (OP_1 0x4e73)
const P2A_SCRIPT = new Uint8Array([0x51, 0x02, 0x4e, 0x73]);
const P2A = { script: P2A_SCRIPT, amount: 0n };

import { ripemd160 } from "@noble/hashes/legacy.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { base64, hex } from "@scure/base";

import {
  getNetworkHrp,
  getNetworkName,
  resolveArkadeServerUrlByName,
} from "./arkade-network.js";
import { createSdkLogger, type Logger, type LogLevel } from "./logging.js";

function secondsToTimelock(
  seconds: number,
): VHTLC.Options["unilateralClaimDelay"] {
  return { type: "seconds" as const, value: BigInt(seconds) };
}

function parseXOnlyPubKey(pubKeyHex: string): Uint8Array {
  const bytes = hex.decode(pubKeyHex);
  if (bytes.length === 33) return bytes.slice(1);
  if (bytes.length === 32) return bytes;
  throw new Error(`Invalid public key length: ${bytes.length}`);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

export interface DelegateClaimParams {
  userSecretKey: string;
  userPubKey: string;
  lendaswapPubKey: string;
  arkadeServerPubKey: string;
  preimage: string;
  preimageHash: string;
  vhtlcAddress: string;
  refundLocktime: number;
  unilateralClaimDelay: number;
  unilateralRefundDelay: number;
  unilateralRefundWithoutReceiverDelay: number;
  /** Destination Arkade address */
  destinationAddress: string;
  network: string;
  /** Lendaswap API base URL (e.g. http://localhost:3333) */
  lendaswapApiUrl: string;
  arkadeServerUrl?: string;
  /** Optional swap ID — enables the backend to mark swap as ClientRedeemed. */
  swapId?: string;
  /** Optional logger sink. Silent by default. */
  logger?: Logger;
  /** Minimum log level to emit. Defaults to `silent`. */
  logLevel?: LogLevel;
}

export interface DelegateRefundParams {
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
  lendaswapApiUrl: string;
  arkadeServerUrl?: string;
  /** Optional logger sink. Silent by default. */
  logger?: Logger;
  /** Minimum log level to emit. Defaults to `silent`. */
  logLevel?: LogLevel;
}

export interface DelegateSettleResult {
  commitmentTxid: string;
}

/**
 * Fetch the backend's static delegate cosigner public key.
 */
export async function fetchCosignerPk(
  lendaswapApiUrl: string,
): Promise<string> {
  const url = `${lendaswapApiUrl.replace(/\/$/, "")}/api/delegate/cosigner-pk`;
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(
      `Failed to fetch cosigner pk: ${res.status} ${await res.text()}`,
    );
  }
  const body = (await res.json()) as { cosigner_pk: string };
  return body.cosigner_pk;
}

/**
 * Settle a VHTLC via delegate claim (reveal preimage).
 */
export async function delegateClaim(
  params: DelegateClaimParams,
): Promise<DelegateSettleResult> {
  const userPkBytes = parseXOnlyPubKey(params.userPubKey);
  const lendaswapPkBytes = parseXOnlyPubKey(params.lendaswapPubKey);
  const serverPkBytes = parseXOnlyPubKey(params.arkadeServerPubKey);

  const preimageBytes = hex.decode(params.preimage);
  const preimageHashBytes = ripemd160(sha256(preimageBytes));

  if (
    hex.encode(preimageHashBytes) !==
    hex.encode(ripemd160(hex.decode(params.preimageHash)))
  ) {
    throw new Error("Preimage hash mismatch");
  }

  // Build VHTLC — for claim: lendaswap=sender, user=receiver
  const networkName = getNetworkName(params.network);
  const vhtlc = new VHTLC.Script({
    sender: lendaswapPkBytes,
    receiver: userPkBytes,
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
  const computedAddr = vhtlc.address(hrp, serverPkBytes).encode();
  if (computedAddr !== params.vhtlcAddress) {
    throw new Error(
      `VHTLC address mismatch: computed ${computedAddr}, expected ${params.vhtlcAddress}`,
    );
  }

  return settleDelegate({
    userSecretKey: params.userSecretKey,
    tapLeafScript: vhtlc.claim(),
    tapTree: vhtlc.encode(),
    vhtlcPkScript: hex.encode(vhtlc.pkScript),
    witnessData: preimageBytes,
    destinationAddress: params.destinationAddress,
    networkName,
    lendaswapApiUrl: params.lendaswapApiUrl,
    arkadeServerUrl: params.arkadeServerUrl,
    locktime: undefined,
    swapId: params.swapId,
    preimage: params.preimage,
    logger: params.logger,
    logLevel: params.logLevel,
  });
}

/**
 * Settle a VHTLC via delegate refund (after locktime expiry).
 */
export async function delegateRefund(
  params: DelegateRefundParams,
): Promise<DelegateSettleResult> {
  const userPkBytes = parseXOnlyPubKey(params.userPubKey);
  const lendaswapPkBytes = parseXOnlyPubKey(params.lendaswapPubKey);
  const serverPkBytes = parseXOnlyPubKey(params.arkadeServerPubKey);

  const hashLockBytes = hex.decode(params.hashLock);
  const preimageHashBytes =
    hashLockBytes.length === 32 ? ripemd160(hashLockBytes) : hashLockBytes;

  // Build VHTLC — for refund: user=sender, lendaswap=receiver
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
  const computedAddr = vhtlc.address(hrp, serverPkBytes).encode();
  if (computedAddr !== params.vhtlcAddress) {
    throw new Error(
      `VHTLC address mismatch: computed ${computedAddr}, expected ${params.vhtlcAddress}`,
    );
  }

  return settleDelegate({
    userSecretKey: params.userSecretKey,
    tapLeafScript: vhtlc.refundWithoutReceiver(),
    tapTree: vhtlc.encode(),
    vhtlcPkScript: hex.encode(vhtlc.pkScript),
    witnessData: undefined,
    destinationAddress: params.destinationAddress,
    networkName,
    lendaswapApiUrl: params.lendaswapApiUrl,
    arkadeServerUrl: params.arkadeServerUrl,
    locktime: params.refundLocktime,
    logger: params.logger,
    logLevel: params.logLevel,
  });
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

interface SettleDelegateOpts {
  userSecretKey: string;
  tapLeafScript: TapLeafScript;
  tapTree: Uint8Array;
  vhtlcPkScript: string;
  witnessData: Uint8Array | undefined;
  destinationAddress: string;
  networkName: NetworkName;
  lendaswapApiUrl: string;
  arkadeServerUrl: string | undefined;
  locktime: number | undefined;
  swapId?: string;
  preimage?: string;
  logger?: Logger;
  logLevel?: LogLevel;
}

async function settleDelegate(
  opts: SettleDelegateOpts,
): Promise<DelegateSettleResult> {
  const {
    userSecretKey,
    tapLeafScript,
    tapTree,
    vhtlcPkScript,
    witnessData,
    destinationAddress,
    networkName,
    lendaswapApiUrl,
    arkadeServerUrl,
  } = opts;

  const logger = createSdkLogger(opts).child({
    module: "delegate",
    operation: "delegate.settle",
    swapId: opts.swapId,
    data: { destinationAddress, networkName },
  });

  const serverUrl = resolveArkadeServerUrlByName(networkName, arkadeServerUrl);

  const arkProvider: ArkProvider = new RestArkProvider(serverUrl);
  const indexerProvider: IndexerProvider = new RestIndexerProvider(serverUrl);
  const serverInfo = await arkProvider.getInfo();

  // Fetch cosigner pk from lendaswap backend
  const cosignerPkHex = await fetchCosignerPk(lendaswapApiUrl);

  // Decode forfeit address (bech32) to pkScript
  const btcNetwork = networks[networkName];
  const forfeitDecoded = Address(btcNetwork).decode(serverInfo.forfeitAddress);
  const forfeitPkScript = OutScript.encode(forfeitDecoded);

  // Fetch VTXOs — include all (not just spendable)
  const { vtxos: allVtxos } = await indexerProvider.getVtxos({
    scripts: [vhtlcPkScript],
  });

  // Filter to unspent VTXOs
  const vtxos = allVtxos.filter((v) => !v.isSpent);

  if (vtxos.length === 0) {
    throw new Error("No settleable VTXOs found at the VHTLC address");
  }

  const totalAmount = vtxos.reduce((acc, v) => acc + BigInt(v.value), 0n);
  if (totalAmount === 0n) {
    throw new Error("Total VTXO amount is zero");
  }

  logger.info({
    event: "delegate.settle.vtxos_found",
    message: "Found settleable VTXOs for delegate settlement",
    data: { vtxoCount: vtxos.length, totalAmount },
  });

  // Parse destination (Ark address → taproot pkScript)
  const destAddr = ArkAddress.decode(destinationAddress);
  const destPkScript = destAddr.pkScript;

  // Build intent message
  const now = Math.floor(Date.now() / 1000);
  const intentMessage: Intent.RegisterMessage = {
    type: "register",
    onchain_output_indexes: [],
    valid_at: now,
    expire_at: now + 120,
    cosigners_public_keys: [cosignerPkHex],
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
    tapLeafScript: [tapLeafScript],
    sequence: opts.locktime ? 0xfffffffe : undefined,
    sighashType: SigHash.ALL,
  }));

  // Build intent proof PSBT
  const intentProof = Intent.create(intentMessage, intentInputs, [
    { script: destPkScript, amount: totalAmount },
  ]);

  // Set VtxoTaprootTree on each real input (skip input 0 which is the toSpend ref)
  for (let i = 0; i < vtxos.length; i++) {
    setArkPsbtField(intentProof, i + 1, VtxoTaprootTree, tapTree);
  }

  // Sign intent proof
  const signer = SingleKey.fromHex(userSecretKey);

  // Set condition witness (preimage) if claiming
  if (witnessData) {
    for (let i = 0; i < vtxos.length; i++) {
      setArkPsbtField(intentProof, i + 1, ConditionWitness, [witnessData]);
    }
  }

  const signedIntentProof = await signer.sign(intentProof);

  // Build and sign delegate forfeit PSBTs.
  // Delegate forfeits have 1 input (the VTXO) with SIGHASH_ALL|ANYONECANPAY.
  // The connector input is added later by the cosigner during batch finalization.
  // Output amount = vtxo_amount + connector_dust (anticipating the connector).
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

    // Main output: VTXO amount + connector dust → forfeit address
    forfeitTx.addOutput({
      script: forfeitPkScript,
      amount: vtxoAmount + dust,
    });

    // Anchor output (P2A)
    forfeitTx.addOutput(P2A);

    // Set taproot tree
    setArkPsbtField(forfeitTx, 0, VtxoTaprootTree, tapTree);

    if (witnessData) {
      setArkPsbtField(forfeitTx, 0, ConditionWitness, [witnessData]);
    }

    const signedForfeit = await signer.sign(forfeitTx);
    signedForfeitPsbts.push(base64.encode(signedForfeit.toPSBT()));
  }

  // Serialize intent proof
  const intentProofBase64 = base64.encode(signedIntentProof.toPSBT());
  const intentMessageJson = Intent.encodeMessage(intentMessage);

  // POST to backend
  const settleUrl = `${lendaswapApiUrl.replace(/\/$/, "")}/api/delegate/settle`;
  const settleRes = await fetch(settleUrl, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      intent_proof: intentProofBase64,
      intent_message: intentMessageJson,
      forfeit_psbts: signedForfeitPsbts,
      cosigner_pk: cosignerPkHex,
      swap_id: opts.swapId,
      preimage: opts.preimage,
    }),
  });

  if (!settleRes.ok) {
    const errBody = await settleRes.text();
    throw new Error(`Delegate settle failed: ${settleRes.status} ${errBody}`);
  }

  const result = (await settleRes.json()) as { commitment_txid: string };
  return { commitmentTxid: result.commitment_txid };
}
