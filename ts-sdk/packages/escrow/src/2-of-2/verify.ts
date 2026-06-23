import { ArkAddress, P2A, type Transaction } from "@arkade-os/sdk";
import { hex } from "@scure/base";

export interface ReleaseArkTxExpectations {
  /** The escrow VTXO outpoint that funds the release. */
  escrowOutpoint: { txid: string; vout: number };
  /** Buyer's payout Arkade address (committed to during take). */
  buyerArkAddress: string;
  /** Buyer payout amount in sats. */
  buyerAmountSats: bigint;
  /** Escrow fee Arkade address. */
  feeArkAddress: string;
  /** Escrow fee amount in sats. */
  feeAmountSats: bigint;
}

/** The release as the seller receives it: the Arkade transaction and its checkpoint(s). */
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
 * funding VTXO, and the **Arkade transaction** spends the checkpoint and pays the final
 * outputs (plus a zero-value P2A fee-bump anchor). This verifies the whole
 * chain the seller is about to sign:
 *   - the checkpoint spends the escrow funding outpoint,
 *   - the Arkade transaction spends that checkpoint,
 *   - the Arkade transaction pays the agreed buyer and fee amounts — and nothing else
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

  // (2) The Arkade transaction must spend that checkpoint.
  if (arkTx.inputsLength !== 1) {
    throw new ReleaseArkTxValidationError(
      `expected exactly 1 Arkade transaction input, got ${arkTx.inputsLength}`,
    );
  }
  const arkIn = arkTx.getInput(0);
  if (!arkIn.txid) {
    throw new ReleaseArkTxValidationError(
      "Arkade transaction input 0 missing prevout",
    );
  }
  const arkInTxid = hex.encode(arkIn.txid);
  if (arkInTxid !== checkpoint.id) {
    throw new ReleaseArkTxValidationError(
      `Arkade transaction input ${arkInTxid} does not spend checkpoint ${checkpoint.id}`,
    );
  }

  // (3) The Arkade transaction must pay buyer + fee, and nothing else but the anchor.
  // A zero (or non-positive) fee is omitted at build time, so we only expect a
  // fee output when the agreed fee is positive — mirroring buildEscrowReleaseTx.
  const buyerAddress = ArkAddress.decode(expected.buyerArkAddress);
  const expectFee = expected.feeAmountSats > 0n;
  const feeAddress = expectFee
    ? ArkAddress.decode(expected.feeArkAddress)
    : undefined;
  const anchorScriptHex = hex.encode(P2A.script);

  let buyerOutputs = 0;
  let feeOutputs = 0;
  let anchorOutputs = 0;

  for (let i = 0; i < arkTx.outputsLength; i++) {
    const output = arkTx.getOutput(i);
    if (!output.script || output.amount === undefined) {
      throw new ReleaseArkTxValidationError(
        `Arkade transaction output ${i} missing script or amount`,
      );
    }

    if (
      matchesAddress(output.script, buyerAddress) &&
      output.amount === expected.buyerAmountSats
    ) {
      buyerOutputs++;
    } else if (
      feeAddress &&
      matchesAddress(output.script, feeAddress) &&
      output.amount === expected.feeAmountSats
    ) {
      feeOutputs++;
    } else if (
      hex.encode(output.script) === anchorScriptHex &&
      output.amount === P2A.amount
    ) {
      anchorOutputs++;
    } else {
      throw new ReleaseArkTxValidationError(
        `unexpected Arkade transaction output ${i}: ${output.amount} sats to ${hex.encode(output.script)}`,
      );
    }
  }

  if (buyerOutputs !== 1) {
    throw new ReleaseArkTxValidationError(
      `expected exactly 1 buyer output paying ${expected.buyerAmountSats} sats to ${expected.buyerArkAddress}, got ${buyerOutputs}`,
    );
  }
  const expectedFeeOutputs = expectFee ? 1 : 0;
  if (feeOutputs !== expectedFeeOutputs) {
    throw new ReleaseArkTxValidationError(
      `expected exactly ${expectedFeeOutputs} fee output(s) paying ${expected.feeAmountSats} sats to ${expected.feeArkAddress}, got ${feeOutputs}`,
    );
  }
  if (anchorOutputs !== 1) {
    throw new ReleaseArkTxValidationError(
      `expected exactly 1 P2A anchor output, got ${anchorOutputs}`,
    );
  }
}

/** Match a pkScript against an Arkade address, allowing its sub-dust form. */
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
