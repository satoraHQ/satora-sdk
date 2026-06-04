import {
  ArkAddress,
  type ArkInfo,
  type ArkProvider,
  type ArkTxInput,
  buildOffchainTx,
  CSVMultisigTapscript,
  combineTapscriptSigs,
  type RelativeTimelock,
  Transaction,
} from "@arkade-os/sdk";
import { base64, hex } from "@scure/base";
import type { EscrowVtxoScript } from "./escrow-script.js";

/**
 * ASP-derived parameters needed to build and submit an escrow release.
 *
 * Derive once at startup from `ArkProvider.getInfo()` via
 * {@link escrowArkConfigFromInfo}; these values are fixed by the ASP and
 * cannot be chosen by the escrow parties.
 */
export interface EscrowArkConfig {
  /** ASP x-only pubkey (32 bytes). */
  aspPubKey: Uint8Array;
  /** ASP-mandated unilateral-exit timelock (the escrow escape-leaf CSV). */
  exitTimelock: RelativeTimelock;
  /** CSV+multisig tapscript used as the second leaf of every checkpoint VTXO. */
  serverUnrollScript: CSVMultisigTapscript.Type;
  /** ASP dust threshold (sats). Sub-dust outputs use an OP_RETURN-shaped script. */
  dust: bigint;
}

/**
 * Derive {@link EscrowArkConfig} from the ASP's `getInfo()` response.
 *
 * Mirrors the connect step every party performs: x-only the signer key,
 * decode the checkpoint tapscript, and map the unilateral-exit delay to a
 * relative timelock.
 */
export function escrowArkConfigFromInfo(info: ArkInfo): EscrowArkConfig {
  if (!info.checkpointTapscript) {
    throw new Error("ASP info is missing checkpointTapscript");
  }
  return {
    aspPubKey: toXOnly(hex.decode(info.signerPubkey)),
    exitTimelock: delayToTimelock(info.unilateralExitDelay),
    serverUnrollScript: CSVMultisigTapscript.decode(
      hex.decode(info.checkpointTapscript),
    ),
    dust: info.dust,
  };
}

/** The escrow funding VTXO being spent by the release. */
export interface EscrowFundingOutpoint {
  txid: string;
  vout: number;
  valueSats: number;
}

/** The two release outputs: buyer payout and escrow fee. */
export interface EscrowReleaseOutputs {
  /** Buyer's payout Ark address (committed to at take time). */
  buyerArkAddress: string;
  /** Sats paid to the buyer. */
  buyerAmountSats: number;
  /** Fee-collection Ark address. */
  feeArkAddress: string;
  /** Sats paid to the fee output. */
  feeSats: number;
}

export interface BuiltEscrowRelease {
  arkTx: Transaction;
  checkpoints: Transaction[];
}

/**
 * Build the cooperative release ark-tx and its checkpoint(s) spending the
 * escrow funding VTXO to `buyer` and `fee` outputs via the cooperative leaf.
 *
 * Deterministic: identical inputs produce identical PSBT bytes and txids, so
 * the arbiter can rebuild on a later round instead of persisting state.
 */
export function buildEscrowReleaseTx(
  escrow: EscrowVtxoScript,
  funding: EscrowFundingOutpoint,
  outputs: EscrowReleaseOutputs,
  config: EscrowArkConfig,
): BuiltEscrowRelease {
  const arkInput: ArkTxInput = {
    txid: funding.txid,
    vout: funding.vout,
    value: funding.valueSats,
    tapLeafScript: escrow.cooperativeLeaf(),
    tapTree: escrow.encode(),
  };

  const buyerAmount = BigInt(outputs.buyerAmountSats);
  const feeAmount = BigInt(outputs.feeSats);
  const buyerAddress = ArkAddress.decode(outputs.buyerArkAddress);
  const feeAddress = ArkAddress.decode(outputs.feeArkAddress);

  const { arkTx, checkpoints } = buildOffchainTx(
    [arkInput],
    [
      {
        script: pkScriptFor(buyerAddress, buyerAmount, config.dust),
        amount: buyerAmount,
      },
      {
        script: pkScriptFor(feeAddress, feeAmount, config.dust),
        amount: feeAmount,
      },
    ],
    config.serverUnrollScript,
  );

  return { arkTx, checkpoints };
}

/**
 * Deterministic auxRand so signing the same PSBT twice yields the same
 * signature. Required because the arbiter rebuilds and re-signs the release
 * across rounds, and `combineTapscriptSigs` rejects a second, differing
 * signature at the same (pubkey, leafHash) slot.
 */
const DETERMINISTIC_AUX_RAND = new Uint8Array(32);

/**
 * Sign input 0 of the ark-tx and every checkpoint with `secretKey`, in place.
 *
 * Defaults to a deterministic auxRand (see above). Used by the arbiter, who
 * signs across rounds; a single-shot signer (the seller) can use
 * {@link signEscrowArkTx} on the base64 PSBTs instead.
 */
export function signEscrowReleaseInPlace(
  built: BuiltEscrowRelease,
  secretKey: Uint8Array,
  auxRand: Uint8Array = DETERMINISTIC_AUX_RAND,
): void {
  built.arkTx.signIdx(secretKey, 0, undefined, auxRand);
  for (const checkpoint of built.checkpoints) {
    checkpoint.signIdx(secretKey, 0, undefined, auxRand);
  }
}

/**
 * Submit the fully-signed ark-tx with UNSIGNED checkpoints to the ASP, merge
 * the user (seller+arbiter) checkpoint sigs into the ASP-signed responses, and
 * finalize. Returns the arkTxid.
 *
 * The submit/finalize split lets the ASP reject a malformed ark-tx before any
 * checkpoint signature is spent. Crash recovery between submit and finalize is
 * the caller's responsibility.
 *
 * @param provider ASP provider used to submit and finalize the ark-tx
 * @param fullySignedArkTx ark-tx carrying both arbiter and seller tapscript sigs
 * @param userSignedCheckpoints checkpoints with arbiter+seller sigs (kept aside)
 * @param unsignedCheckpoints fresh-from-build checkpoints (no signatures)
 */
export async function submitAndFinalizeEscrowRelease(
  provider: ArkProvider,
  fullySignedArkTx: Transaction,
  userSignedCheckpoints: Transaction[],
  unsignedCheckpoints: Transaction[],
): Promise<string> {
  const { arkTxid, signedCheckpointTxs } = await provider.submitTx(
    base64.encode(fullySignedArkTx.toPSBT()),
    unsignedCheckpoints.map((c) => base64.encode(c.toPSBT())),
  );

  const finalCheckpoints = signedCheckpointTxs.map((c, i) => {
    const aspSigned = Transaction.fromPSBT(base64.decode(c));
    const userSigned = userSignedCheckpoints[i];
    if (!userSigned) {
      throw new Error(`missing user-signed checkpoint at index ${i}`);
    }
    combineTapscriptSigs(userSigned, aspSigned);
    return base64.encode(aspSigned.toPSBT());
  });

  await provider.finalizeTx(arkTxid, finalCheckpoints);
  return arkTxid;
}

/**
 * The ASP rejects any non-OP_RETURN output below `dust`. Ark addresses expose
 * a {@link ArkAddress.subdustPkScript} encoding the destination as an
 * OP_RETURN-shaped script for amounts the recipient still wants but that are
 * sub-dust on L1 (e.g. a 1-sat fee on a small trade).
 */
function pkScriptFor(
  address: ArkAddress,
  amount: bigint,
  dust: bigint,
): Uint8Array {
  return amount < dust ? address.subdustPkScript : address.pkScript;
}

/**
 * BIP-68: a relative-timelock value below 512 is encoded as a block height;
 * otherwise it is a 512-second-granularity time value (rounded up to the next
 * valid multiple). ASP-reported values are already valid.
 */
function delayToTimelock(delay: bigint): RelativeTimelock {
  if (delay < 512n) {
    return { value: delay, type: "blocks" };
  }
  const rounded = ((delay + 511n) / 512n) * 512n;
  return { value: rounded, type: "seconds" };
}

/**
 * Drop the sign byte from a 33-byte compressed secp256k1 pubkey to get the
 * 32-byte x-only form used by BIP-340 / tapscripts. The ASP returns its
 * `signerPubkey` compressed.
 */
function toXOnly(pubkey: Uint8Array): Uint8Array {
  if (pubkey.length === 32) return pubkey;
  if (pubkey.length === 33) return pubkey.subarray(1);
  throw new Error(`unexpected pubkey length ${pubkey.length}`);
}
