import { buildNativeClaimTx } from "../evm/native.js";
import type { EvmSigner } from "../evm/wallet.js";
import {
  getRevertReason,
  simulateTransaction,
  txTypeFor,
} from "../evm/wallet.js";
import type { ClaimGaslessResult } from "./types.js";
import {
  buildRedeemAndExecuteTx,
  type RedeemTxParams,
} from "./userop-claim.js";

export interface SignerClaimParams extends RedeemTxParams {
  /** The wallet that submits the claim and pays its gas. */
  signer: EvmSigner;
}

/**
 * Claims an EVM-targeted swap from the user's own wallet.
 *
 * Same signed `redeemAndExecute` calldata as the sponsored UserOp path, but
 * broadcast by `signer` (a connected wallet) which pays the gas. This is the
 * fallback when no bundler / paymaster is configured or the sponsored claim
 * keeps failing: the swap's derived key still authorises the redeem, the
 * wallet only carries the transaction.
 */
export async function claimViaSigner(
  params: SignerClaimParams,
): Promise<ClaimGaslessResult> {
  const { signer, swap } = params;
  if (signer.chainId !== swap.evm_chain_id) {
    throw new Error(
      `Wallet is on chain ${signer.chainId}, but the claim must be sent on chain ${swap.evm_chain_id}.`,
    );
  }
  const native =
    (swap as { evm_htlc_kind?: string }).evm_htlc_kind === "native";
  const tx = native
    ? buildNativeClaimTx({ ...params, caller: signer.address })
    : buildRedeemAndExecuteTx(params);
  await simulateTransaction(signer, tx, "Claim transaction");
  const txHash = await signer.sendTransaction({
    ...tx,
    ...(native ? { type: txTypeFor(signer.chainId) } : {}),
  });
  const receipt = await signer.waitForReceipt(txHash);
  const minedHash = native ? receipt.transactionHash || txHash : txHash;
  if (receipt.status !== "success") {
    const reason = await getRevertReason(
      signer,
      minedHash,
      receipt.blockNumber,
    );
    throw new Error(`Claim transaction reverted: ${reason}`);
  }
  if (native && minedHash.toLowerCase() !== txHash.toLowerCase()) {
    // Wallet cancellation receipts can also have status "success". Only a
    // replacement of the same authorized claim counts as a completed claim.
    const mined = await signer.getTransaction(minedHash).catch(() => undefined);
    if (!mined) {
      // A lagging RPC cannot distinguish a speed-up from a cancellation.
      // Neither report success nor invite a new claim without verification.
      throw new Error(
        `Replacement transaction ${minedHash} was mined, but its claim could not yet be verified. Check the swap status or transaction before submitting another claim.`,
      );
    }
    if (
      mined.to?.toLowerCase() !== tx.to.toLowerCase() ||
      mined.from.toLowerCase() !== signer.address.toLowerCase() ||
      mined.input.toLowerCase() !== tx.data.toLowerCase()
    ) {
      throw new Error(
        "The wallet replaced or cancelled the claim transaction. The claim was not submitted; try again from the payout wallet.",
      );
    }
  }
  return {
    id: swap.id,
    status: "clientredeemed",
    txHash: minedHash,
    message: `${native ? "Claim" : "redeemAndExecute"} published from wallet ${signer.address}`,
  };
}
