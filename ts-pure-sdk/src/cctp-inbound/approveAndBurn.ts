/**
 * Source-chain USDC approve + burn for the CCTP-inbound flow.
 *
 * Handles the full source-chain leg in one call:
 *   1. Read current allowance via `EvmSigner.call`. If < `amount`,
 *      send an `approve` and wait for its receipt.
 *   2. Call `TokenMessenger.depositForBurn(...)` with `mintRecipient`
 *      and `destinationCaller` both pinned to the caller's
 *      smart-account address.
 *   3. Return the tx hashes.
 *
 * Takes the SDK's `EvmSigner` so the same signer used for direct
 * Permit2 swaps (wagmi, Privy, raw key via the `evm/wallet` adapter)
 * also powers the CCTP flow.
 */

import type { Address, Hex } from "viem";
import { encodeFunctionData, erc20Abi, parseAbi } from "viem";
import { FINALITY_FAST, TOKEN_MESSENGER_V2 } from "../cctp/constants.js";
import { addressToBytes32 } from "../cctp/utils.js";
import type { EvmSigner } from "../evm/wallet.js";

const TOKEN_MESSENGER_ABI = parseAbi([
  "function depositForBurn(uint256 amount, uint32 destinationDomain, bytes32 mintRecipient, address burnToken, bytes32 destinationCaller, uint256 maxFee, uint32 minFinalityThreshold) external returns (uint64)",
]);

export interface ApproveAndBurnParams {
  /** SDK signer bound to the source chain. */
  signer: EvmSigner;
  /** USDC amount in smallest units (6 decimals). */
  amount: bigint;
  /** USDC contract address on the source chain. */
  usdcAddress: Address;
  /** CCTP destination domain id (3 = Arbitrum mainnet). */
  destinationDomain: number;
  /**
   * The address that will both (a) receive the minted USDC on the
   * destination chain and (b) be the sole caller allowed to submit
   * `receiveMessage`. For the CCTP-inbound settlement flow this is
   * the caller's Kernel smart-account address.
   */
  smartAccountAddress: Address;
  /**
   * Max CCTPv2 fast-transfer fee in USDC units, from the IRIS fee API.
   * The caller is responsible for fetching this — typical value is
   * ~1/10000 of the amount.
   */
  maxFee: bigint;
  /**
   * CCTP finality threshold. Defaults to fast transfer (1000).
   * Use 2000 for standard (cheaper, ~13 minutes) transfers.
   */
  minFinalityThreshold?: number;
  /**
   * Override the TokenMessenger contract address. Defaults to the
   * canonical CCTPv2 deployment shared across all EVM chains.
   */
  tokenMessengerAddress?: Address;
  /**
   * When `true`, always send `approve` even if existing allowance is
   * sufficient. Defaults to `false` to avoid unnecessary txs.
   */
  forceApprove?: boolean;
}

export interface ApproveAndBurnResult {
  /** Tx hash of the USDC approval; omitted when the existing allowance was sufficient. */
  approveTxHash?: Hex;
  /** Tx hash of the `depositForBurn` call. */
  burnTxHash: Hex;
}

/**
 * Approve USDC to the TokenMessenger (if needed) then burn it for the
 * cross-chain transfer. Returns the tx hashes. Does NOT wait for the
 * burn receipt — the caller pairs this with `waitForAttestation`.
 */
export async function approveAndBurn(
  params: ApproveAndBurnParams,
): Promise<ApproveAndBurnResult> {
  const {
    signer,
    amount,
    usdcAddress,
    destinationDomain,
    smartAccountAddress,
    maxFee,
    minFinalityThreshold = FINALITY_FAST,
    tokenMessengerAddress = TOKEN_MESSENGER_V2 as Address,
    forceApprove = false,
  } = params;

  // 1. Allowance check — skip `approve` if the user already granted
  //    enough USDC to the TokenMessenger. Saves a tx + signature
  //    prompt on repeat swaps.
  let approveTxHash: Hex | undefined;
  const needsApprove =
    forceApprove ||
    !(await hasEnoughAllowance({
      signer,
      usdcAddress,
      spender: tokenMessengerAddress,
      minAmount: amount,
    }));

  if (needsApprove) {
    approveTxHash = (await signer.sendTransaction({
      to: usdcAddress,
      data: encodeFunctionData({
        abi: erc20Abi,
        functionName: "approve",
        args: [tokenMessengerAddress, amount],
      }),
    })) as Hex;
    // Wait for the approve to mine — otherwise depositForBurn sees a
    // stale allowance and reverts.
    const receipt = await signer.waitForReceipt(approveTxHash);
    if (receipt.status !== "success") {
      throw new Error(`USDC approve reverted: tx ${approveTxHash}`);
    }
  }

  // 2. Burn. `mintRecipient` and `destinationCaller` both pinned to
  //    the smart-account address:
  //      - Destination USDC mints into the smart account (not the EOA).
  //      - Only that account can call `receiveMessage` on the
  //        destination chain → blocks front-runners from drawing gas
  //        from our paymaster via a racing UserOp.
  const recipientBytes32 = addressToBytes32(smartAccountAddress) as Hex;
  const burnTxHash = (await signer.sendTransaction({
    to: tokenMessengerAddress,
    data: encodeFunctionData({
      abi: TOKEN_MESSENGER_ABI,
      functionName: "depositForBurn",
      args: [
        amount,
        destinationDomain,
        recipientBytes32,
        usdcAddress,
        recipientBytes32,
        maxFee,
        minFinalityThreshold,
      ],
    }),
  })) as Hex;

  return { approveTxHash, burnTxHash };
}

/** ERC-20 `allowance(owner, spender)` read via `EvmSigner.call`. */
async function hasEnoughAllowance(args: {
  signer: EvmSigner;
  usdcAddress: Address;
  spender: Address;
  minAmount: bigint;
}): Promise<boolean> {
  const { signer, usdcAddress, spender, minAmount } = args;
  const data = encodeFunctionData({
    abi: erc20Abi,
    functionName: "allowance",
    args: [signer.address as Address, spender],
  });
  const result = await signer.call({ to: usdcAddress, data });
  // `allowance` returns uint256 — 32 bytes, big-endian hex.
  const hex = result.replace(/^0x/, "") || "0";
  const current = BigInt(`0x${hex}`);
  return current >= minAmount;
}
