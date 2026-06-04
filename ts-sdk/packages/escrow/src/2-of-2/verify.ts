import { ArkAddress, P2A, type Transaction } from "@arkade-os/sdk";
import { hex } from "@scure/base";

export interface ReleaseArkTxExpectations {
  /** The escrow VTXO outpoint that funds the release. */
  escrowOutpoint: { txid: string; vout: number };
  /** Buyer's payout Ark address (committed to during take). */
  buyerArkAddress: string;
  /** Buyer payout amount in sats. */
  buyerAmountSats: bigint;
  /** Escrow fee Ark address. */
  feeArkAddress: string;
  /** Escrow fee amount in sats. */
  feeAmountSats: bigint;
}

/** The release as the seller receives it: the ark-tx and its checkpoint(s). */
export interface ReleaseToVerify {
  arkTx: Transaction;
  checkpoints: Transaction[];
}

export class ReleaseArkTxValidationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ReleaseArkTxValidationError";
  }
}

/**
 * Seller-side check before signing the cooperative release.
 *
 * An Arkade offchain spend is a two-tx chain: a **checkpoint** spends the
 * funding VTXO, and the **ark-tx** spends the checkpoint and pays the final
 * outputs (plus a zero-value P2A fee-bump anchor). This verifies the whole
 * chain the seller is about to sign:
 *   - the checkpoint spends the escrow funding outpoint,
 *   - the ark-tx spends that checkpoint,
 *   - the ark-tx pays the agreed buyer and fee amounts — and nothing else
 *     except the anchor (no rogue payout).
 *
 * Throws on any mismatch. Caller must NOT sign if this throws.
 */
export function verifyReleaseArkTx(
  release: ReleaseToVerify,
  expected: ReleaseArkTxExpectations,
): void {
  const { arkTx, checkpoints } = release;

  // A single funding VTXO yields exactly one checkpoint.
  if (checkpoints.length !== 1) {
    throw new ReleaseArkTxValidationError(
      `expected exactly 1 checkpoint, got ${checkpoints.length}`,
    );
  }
  const checkpoint = checkpoints[0];

  // (1) The checkpoint must spend the escrow funding outpoint.
  const cpIn = checkpoint.getInput(0);
  if (!cpIn.txid || cpIn.index === undefined) {
    throw new ReleaseArkTxValidationError("checkpoint input 0 missing prevout");
  }
  const cpInTxid = hex.encode(cpIn.txid);
  if (
    cpInTxid !== expected.escrowOutpoint.txid ||
    cpIn.index !== expected.escrowOutpoint.vout
  ) {
    throw new ReleaseArkTxValidationError(
      `checkpoint input ${cpInTxid}:${cpIn.index} does not spend funding ${expected.escrowOutpoint.txid}:${expected.escrowOutpoint.vout}`,
    );
  }

  // (2) The ark-tx must spend that checkpoint.
  if (arkTx.inputsLength !== 1) {
    throw new ReleaseArkTxValidationError(
      `expected exactly 1 ark-tx input, got ${arkTx.inputsLength}`,
    );
  }
  const arkIn = arkTx.getInput(0);
  if (!arkIn.txid) {
    throw new ReleaseArkTxValidationError("ark-tx input 0 missing prevout");
  }
  const arkInTxid = hex.encode(arkIn.txid);
  if (arkInTxid !== checkpoint.id) {
    throw new ReleaseArkTxValidationError(
      `ark-tx input ${arkInTxid} does not spend checkpoint ${checkpoint.id}`,
    );
  }

  // (3) The ark-tx must pay buyer + fee, and nothing else but the anchor.
  const buyerAddress = ArkAddress.decode(expected.buyerArkAddress);
  const feeAddress = ArkAddress.decode(expected.feeArkAddress);
  const anchorScriptHex = hex.encode(P2A.script);

  let foundBuyer = false;
  let foundFee = false;

  for (let i = 0; i < arkTx.outputsLength; i++) {
    const output = arkTx.getOutput(i);
    if (!output.script || output.amount === undefined) {
      throw new ReleaseArkTxValidationError(
        `ark-tx output ${i} missing script or amount`,
      );
    }

    if (
      matchesAddress(output.script, buyerAddress) &&
      output.amount === expected.buyerAmountSats
    ) {
      foundBuyer = true;
    } else if (
      matchesAddress(output.script, feeAddress) &&
      output.amount === expected.feeAmountSats
    ) {
      foundFee = true;
    } else if (
      hex.encode(output.script) === anchorScriptHex &&
      output.amount === P2A.amount
    ) {
      // Zero-value P2A fee-bump anchor — expected.
    } else {
      throw new ReleaseArkTxValidationError(
        `unexpected ark-tx output ${i}: ${output.amount} sats to ${hex.encode(output.script)}`,
      );
    }
  }

  if (!foundBuyer) {
    throw new ReleaseArkTxValidationError(
      `no output paying ${expected.buyerAmountSats} sats to buyer ${expected.buyerArkAddress}`,
    );
  }
  if (!foundFee) {
    throw new ReleaseArkTxValidationError(
      `no output paying ${expected.feeAmountSats} sats to fee ${expected.feeArkAddress}`,
    );
  }
}

/** Match a pkScript against an Ark address, allowing its sub-dust form. */
function matchesAddress(script: Uint8Array, address: ArkAddress): boolean {
  return (
    bytesEqual(script, address.pkScript) ||
    bytesEqual(script, address.subdustPkScript)
  );
}

function bytesEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}
