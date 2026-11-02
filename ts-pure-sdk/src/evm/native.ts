/** Direct calls to the deployed native HTLC. No coordinator is involved. */
import { type Address, encodeFunctionData, type Hex, parseAbi } from "viem";
import {
  buildNativeRedeemDigest,
  computeCoordinatorCallsHash,
  NATIVE_TOKEN_ADDRESS,
} from "./coordinator.js";
import { normalizeBytes32 } from "./htlc.js";
import { signEvmDigest } from "./signing.js";

const abi = parseAbi([
  "function create(bytes32 preimageHash, address claimAddress, uint256 timelock) payable",
  "function redeemBySig(bytes32 preimage, uint256 amount, address sender, uint256 timelock, address destination, address sweepToken, uint256 minAmountOut, bytes32 callsHash, uint8 v, bytes32 r, bytes32 s) returns (address)",
]);

export function encodeNativeCreate(params: {
  htlcAddress: string;
  preimageHash: string;
  claimAddress: string;
  timelock: number;
  amount: bigint;
}): { to: string; data: string; value: bigint } {
  return {
    to: params.htlcAddress,
    data: encodeFunctionData({
      abi,
      functionName: "create",
      args: [
        `0x${normalizeBytes32(params.preimageHash)}` as Hex,
        params.claimAddress as Address,
        BigInt(params.timelock),
      ],
    }),
    value: params.amount,
  };
}

/** The deployed HTLC pays msg.sender; caller MUST be the intended recipient. */
export function buildNativeClaimTx(params: {
  preimage: string;
  secretKey: Uint8Array;
  caller: string;
  destination: string;
  swap: {
    evm_htlc_address: string;
    evm_chain_id: number;
    evm_htlc_version: number;
    evm_expected_sats: string | number;
    server_evm_address: string;
    evm_refund_locktime: number;
  };
}): { to: string; data: string } {
  const { swap, caller, destination, secretKey } = params;
  if (caller.toLowerCase() !== destination.toLowerCase()) {
    throw new Error(
      "Connect the payout wallet to claim RBTC: the native HTLC pays the submitting wallet.",
    );
  }
  const preimage = (
    params.preimage.startsWith("0x") ? params.preimage : `0x${params.preimage}`
  ) as Hex;
  const amount = BigInt(swap.evm_expected_sats);
  const callsHash = computeCoordinatorCallsHash([]) as Hex;
  const signature = signEvmDigest(
    secretKey,
    buildNativeRedeemDigest({
      htlcAddress: swap.evm_htlc_address,
      chainId: swap.evm_chain_id,
      htlcVersion: swap.evm_htlc_version,
      preimage,
      amount,
      sender: swap.server_evm_address,
      timelock: swap.evm_refund_locktime,
      caller,
      destination,
      sweepToken: NATIVE_TOKEN_ADDRESS,
      minAmountOut: amount,
      callsHash,
    }),
  );
  return {
    to: swap.evm_htlc_address,
    data: encodeFunctionData({
      abi,
      functionName: "redeemBySig",
      args: [
        preimage,
        amount,
        swap.server_evm_address as Address,
        BigInt(swap.evm_refund_locktime),
        destination as Address,
        NATIVE_TOKEN_ADDRESS,
        amount,
        callsHash,
        signature.v,
        signature.r as Hex,
        signature.s as Hex,
      ],
    }),
  };
}
