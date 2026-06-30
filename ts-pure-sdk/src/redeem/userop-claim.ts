/**
 * Claim a BTC/Arkade/Lightning → EVM swap by publishing
 * `coordinator.redeemAndExecute` **client-side** as a paymaster-sponsored
 * EIP-7702 UserOp, instead of asking the server to submit it.
 *
 * Why: the server-submitted path builds the DEX leg with a fixed gas limit and
 * the legacy (Uniswap) router, which breaks LI.FI-only targets like EURe. Here
 * the SDK fetches LI.FI calldata from `/dex-calldata`, signs the redeem digest
 * with the swap's own derived key, and the Alchemy bundler simulates+sponsors
 * the UserOp — so gas is estimated (no OOG) and the user needs no ETH.
 *
 * The publisher is the swap's *derived* EVM key (it already signs the EIP-712
 * redeem authorization), wrapped as a 7702 smart account — fully self-contained.
 */

import { type Hex, toHex } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { arbitrum } from "viem/chains";
import { createSwapSmartAccountClient } from "../cctp-inbound/smartAccount.js";
import type { AaConfig } from "../cctp-inbound/types.js";
import {
  buildRedeemDigest,
  type CoordinatorCall,
  computeCoordinatorCallsHash,
  encodeRedeemAndExecute,
  signEvmDigest,
} from "../evm/index.js";
import type { EvmSigner } from "../evm/wallet.js";
import type { GaslessSwapResponse } from "./gasless.js";
import type { ClaimGaslessResult } from "./types.js";

/** Parameters for a sponsored-UserOp claim. */
export interface UserOpClaimParams {
  /** Swap preimage/secret (hex, with or without 0x). */
  preimage: string;
  /** The swap's derived EVM signing key (raw bytes). */
  secretKey: Uint8Array;
  /** The swap data from the server. */
  swap: GaslessSwapResponse;
  /** EVM address tokens are ultimately swept to. */
  destination: string;
  /**
   * The exact `approve` + swap calls from `POST /dex-calldata/fund` (built with
   * `recipient = coordinator` so the coordinator's `_sweep` can deliver). Used
   * verbatim so the locally-computed `callsHash` matches what we sign.
   */
  calls: CoordinatorCall[];
  /** Slippage floor for the coordinator `_sweep` (smallest target-token units). */
  minAmountOut: bigint;
  /** AA config (bundler URL + Gas Manager policy id) — required for sponsorship. */
  aa: AaConfig;
}

/**
 * Wrap the swap's derived private key as the minimal {@link EvmSigner} the AA
 * client needs: `signMessage` (UserOp hash) + `signAuthorization` (7702 tuple).
 * The other `EvmSigner` methods are unused on the sponsored path and throw.
 */
function derivedKeyToAaSigner(
  secretKey: Uint8Array,
  chainId: number,
): EvmSigner {
  const account = privateKeyToAccount(toHex(secretKey));
  const unsupported = (m: string) => async (): Promise<never> => {
    throw new Error(
      `derivedKeyToAaSigner: ${m} is not supported on the sponsored UserOp path`,
    );
  };
  return {
    address: account.address,
    chainId,
    signMessage: ({ raw }) =>
      account.signMessage({ message: { raw: raw as Hex } }),
    signAuthorization: async (auth) => {
      const signed = await account.signAuthorization({
        chainId: auth.chainId,
        address: auth.contractAddress as Hex,
        nonce: auth.nonce,
      });
      return {
        r: signed.r,
        s: signed.s,
        v: signed.v === undefined ? undefined : Number(signed.v),
        yParity: signed.yParity ?? 0,
        chainId: auth.chainId,
        address: auth.contractAddress,
        nonce: auth.nonce,
      };
    },
    signTypedData: unsupported("signTypedData"),
    sendTransaction: unsupported("sendTransaction"),
    waitForReceipt: unsupported("waitForReceipt"),
    getTransaction: unsupported("getTransaction"),
    call: unsupported("call"),
  };
}

/**
 * Build, sign and publish `redeemAndExecute` as a sponsored UserOp.
 */
export async function claimViaUserOp(
  params: UserOpClaimParams,
): Promise<ClaimGaslessResult> {
  const { preimage, secretKey, swap, destination, calls, minAmountOut, aa } =
    params;

  const secretHex = preimage.startsWith("0x") ? preimage : `0x${preimage}`;
  const wbtcAddress = swap.wbtc_address;
  const amount = BigInt(swap.evm_expected_sats);
  const targetTokenAddress = String(swap.target_token.token_id);
  const sweepToken =
    targetTokenAddress.toLowerCase() !== wbtcAddress.toLowerCase()
      ? targetTokenAddress
      : wbtcAddress;

  // The SDK now owns the calls, so it computes the signed `callsHash` itself
  // (parity-tested against Solidity `abi.encode(Call[])`).
  const callsHash = computeCoordinatorCallsHash(calls);

  const digest = buildRedeemDigest({
    htlcAddress: swap.evm_htlc_address,
    chainId: swap.evm_chain_id,
    preimage: secretHex,
    amount,
    token: wbtcAddress,
    sender: swap.server_evm_address,
    timelock: swap.evm_refund_locktime,
    caller: swap.evm_coordinator_address,
    destination,
    sweepToken,
    minAmountOut,
    callsHash,
  });
  const sig = signEvmDigest(secretKey, digest);

  const { to, data } = encodeRedeemAndExecute(swap.evm_coordinator_address, {
    preimage: secretHex,
    amount,
    token: wbtcAddress,
    sender: swap.server_evm_address,
    timelock: swap.evm_refund_locktime,
    calls,
    sweepToken,
    minAmountOut,
    destination,
    v: sig.v,
    r: sig.r,
    s: sig.s,
  });

  // ── Publish ──────────────────────────────────────────────────────────────
  // Default mode: paymaster-sponsored 7702 UserOp via the Alchemy bundler. The
  // bundler simulates (so gas is estimated, no OOG) and the Gas Manager policy
  // sponsors it, so the swap's derived account needs no ETH.
  //
  // TODO(self-funded claims): to support clients without a sponsoring
  //   paymaster (e.g. third-party integrators publishing from a funded EOA),
  //   branch here on whether `aa` is present:
  //     - sponsored (today): the path below.
  //     - self-funded: take an `EvmSigner` from the caller (their wallet) and
  //       `await signer.sendTransaction({ to, data })` then
  //       `signer.waitForReceipt(hash)`. That publisher must hold native gas;
  //       the coordinator accepts any caller, so no other change is needed.
  const signer = derivedKeyToAaSigner(secretKey, swap.evm_chain_id);
  const { client } = await createSwapSmartAccountClient({
    signer,
    aa,
    chain: arbitrum,
  });

  const userOpHash = await client.sendUserOperation({
    calls: [{ to: to as Hex, data: data as Hex, value: 0n }],
  });
  const receipt = await client.waitForUserOperationReceipt({
    hash: userOpHash,
  });

  return {
    id: swap.id,
    status: "clientredeemed",
    txHash: receipt.receipt.transactionHash,
    message: "redeemAndExecute published via sponsored UserOp",
  };
}
