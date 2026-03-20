/**
 * Escrow signing utilities for ark-escrow PSBT exchange protocol.
 *
 * Provides helpers for signing escrow release/refund PSBTs from the
 * browser or Node.js. The arbiter (backend) builds and pre-signs the
 * PSBTs; these functions let the counterparty (Bob or Alice) add their
 * signature before the arbiter merges and submits to Arkade.
 *
 * @see https://github.com/ArkEscrow/ark-escrow for the full protocol.
 */

import { Transaction } from "@arkade-os/sdk";
import { hex } from "@scure/base";

// -- Base64 helpers (browser-safe, no Buffer dependency) --

function b64Decode(s: string): Uint8Array {
  const bin = atob(s);
  const arr = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
  return arr;
}

function b64Encode(u: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < u.length; i++) bin += String.fromCharCode(u[i]);
  return btoa(bin);
}

/** Result of signing an escrow ark transaction. */
export interface SignedEscrowTx {
  /** The signed PSBT as a base64 string. */
  signedPsbt: string;
  /** The transaction ID (hex) extracted from the PSBT. */
  txid: string;
}

/**
 * Sign the escrow ark transaction PSBT.
 *
 * The ark_tx is the main offchain transaction spending the escrow VTXO.
 * The signer adds a tapscript Schnorr signature at input index 0.
 *
 * @param psbtB64 - The ark_tx PSBT as a base64 string (from the arbiter).
 * @param secretKey - The signer's secret key (32 bytes or 64-char hex string).
 * @returns The signed PSBT (base64) and the transaction ID (hex).
 */
export function signEscrowArkTx(
  psbtB64: string,
  secretKey: Uint8Array | string,
): SignedEscrowTx {
  const sk = typeof secretKey === "string" ? hex.decode(secretKey) : secretKey;
  const tx = Transaction.fromPSBT(b64Decode(psbtB64));
  tx.signIdx(sk, 0);

  const txId = tx.id as unknown;
  const txid = txId instanceof Uint8Array ? hex.encode(txId) : String(txId);

  return {
    signedPsbt: b64Encode(tx.toPSBT()),
    txid,
  };
}

/**
 * Sign escrow checkpoint PSBTs.
 *
 * Checkpoint transactions are signed after the Arkade server co-signs
 * the ark_tx. Each checkpoint is signed at input index 0.
 *
 * @param psbtB64s - Array of checkpoint PSBTs as base64 strings.
 * @param secretKey - The signer's secret key (32 bytes or 64-char hex string).
 * @returns Array of signed checkpoint PSBTs as base64 strings.
 */
export function signEscrowCheckpoints(
  psbtB64s: string[],
  secretKey: Uint8Array | string,
): string[] {
  const sk = typeof secretKey === "string" ? hex.decode(secretKey) : secretKey;
  return psbtB64s.map((cpB64) => {
    const tx = Transaction.fromPSBT(b64Decode(cpB64));
    tx.signIdx(sk, 0);
    return b64Encode(tx.toPSBT());
  });
}

/**
 * Extract the transaction ID from an ark_tx PSBT without signing.
 *
 * Useful when you need the txid for the finalize step but already
 * have the signed PSBT from an earlier step.
 *
 * @param psbtB64 - The ark_tx PSBT as a base64 string.
 * @returns The transaction ID as a hex string.
 */
export function getArkTxid(psbtB64: string): string {
  const tx = Transaction.fromPSBT(b64Decode(psbtB64));
  const txId = tx.id as unknown;
  return txId instanceof Uint8Array ? hex.encode(txId) : String(txId);
}
