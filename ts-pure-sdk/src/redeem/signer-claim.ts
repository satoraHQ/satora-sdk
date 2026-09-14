import type { EvmSigner } from "../evm/wallet.js";
import { getRevertReason, simulateTransaction } from "../evm/wallet.js";
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
  const tx = buildRedeemAndExecuteTx(params);
  await simulateTransaction(signer, tx, "Claim transaction");
  const txHash = await signer.sendTransaction({ to: tx.to, data: tx.data });
  const receipt = await signer.waitForReceipt(txHash);
  if (receipt.status !== "success") {
    const reason = await getRevertReason(signer, txHash, receipt.blockNumber);
    throw new Error(`Claim transaction reverted: ${reason}`);
  }
  return {
    id: swap.id,
    status: "clientredeemed",
    txHash,
    message: `redeemAndExecute published from wallet ${signer.address}`,
  };
}
