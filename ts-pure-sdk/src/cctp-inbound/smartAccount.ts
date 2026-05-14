/**
 * Build a ZeroDev Kernel smart-account client on Arbitrum from a viem
 * `Account` supplied by the consumer (Privy, wagmi, raw private key —
 * any viem-compatible signer).
 *
 * Under EIP-7702 the consumer's EOA *is* the smart account — its code
 * is delegated to a Kernel implementation on the first UserOp. So the
 * smart-account address and the EOA address are the same, which makes
 * the EOA address usable directly as both:
 *   - `mintRecipient` on the source-chain CCTP burn (USDC arrives here)
 *   - `destinationCaller` (bytes32-padded) — only this account can
 *     call `receiveMessage` on Arbitrum
 *
 * No CREATE2, no factory, no counterfactual address: the first
 * UserOperation carries the 7702 authorization tuple and installs the
 * delegation on-chain; subsequent ones reuse it.
 *
 * Bundler + paymaster both live at the same Alchemy app URL; the
 * policy id is passed via the ERC-7677 `paymasterContext`.
 */

import { createKernelAccount, createKernelAccountClient } from "@zerodev/sdk";
import { getEntryPoint, KERNEL_V3_3 } from "@zerodev/sdk/constants";
import type { Chain } from "viem";
import { createPublicClient, http } from "viem";
import { createPaymasterClient } from "viem/account-abstraction";
import { arbitrum } from "viem/chains";
import type { EvmSigner } from "../evm/wallet.js";
import { evmSignerToKernelOwner } from "./evmSignerAdapter.js";
import type { AaConfig } from "./types.js";

export interface CreateSwapSmartAccountClientParams {
  /**
   * The EOA being delegated, expressed as the SDK's `EvmSigner`. The
   * same abstraction used by `Client.fundSwap` — one signer covers
   * both the direct-Permit2 and CCTP-inbound paths. The CCTP flow
   * requires `signer.signMessage` (UserOp hash) **and**
   * `signer.signAuthorization` (7702 auth tuple) — both optional on
   * `EvmSigner`; the adapter throws a clear error if either is missing.
   */
  signer: EvmSigner;
  /** AA config (bundler URL + Gas Manager policy id). */
  aa: AaConfig;
  /**
   * Settlement chain. Defaults to Arbitrum mainnet — the only supported
   * chain today, parameterised here so tests / future chains can plug
   * in without a signature change.
   */
  chain?: Chain;
}

/**
 * Creates a Kernel smart-account client ready to send UserOperations.
 * Async — resolves once the account is derived.
 *
 * @returns `{ client, account, accountAddress }` where `accountAddress`
 *          equals the consumer's EOA address (7702: EOA *is* the account).
 */
export async function createSwapSmartAccountClient(
  params: CreateSwapSmartAccountClientParams,
) {
  const { signer, aa, chain = arbitrum } = params;
  const { bundlerUrl, paymasterPolicyId } = aa;

  if (!bundlerUrl) {
    throw new Error("aa.bundlerUrl is required");
  }
  if (!paymasterPolicyId) {
    throw new Error("aa.paymasterPolicyId is required");
  }

  const entryPoint = getEntryPoint("0.7");

  const publicClient = createPublicClient({
    chain,
    transport: http(bundlerUrl),
  });

  // Adapt the EvmSigner to a viem LocalAccount. Under 7702 this is the
  // EOA whose code gets delegated to Kernel — the adapter throws a
  // clear error if `signMessage` (UserOp hash) or `signAuthorization`
  // (7702 auth tuple) is missing on the EvmSigner.
  const kernelOwner = evmSignerToKernelOwner(signer);

  // EIP-7702 mode: pass the EOA as `eip7702Account`. Kernel signs the
  // authorization tuple on the first UserOp via the account's
  // `signAuthorization`, installing the delegation on-chain. No
  // separate ECDSA validator plugin or CREATE2 factory needed — the
  // delegated EOA's own key gates the account.
  const account = await createKernelAccount(publicClient, {
    eip7702Account: kernelOwner,
    entryPoint,
    kernelVersion: KERNEL_V3_3,
  });

  // Alchemy serves standard ERC-7677 paymaster methods
  // (`pm_getPaymasterStubData` / `pm_getPaymasterData`) on the same app
  // URL as the bundler. Passing the policy id via `paymasterContext`
  // lets viem call both methods at the right points in the UserOp
  // preparation flow (stub → gas estimate → final paymaster data).
  const paymasterClient = createPaymasterClient({
    transport: http(bundlerUrl),
  });

  const client = createKernelAccountClient({
    account,
    chain,
    bundlerTransport: http(bundlerUrl),
    paymaster: paymasterClient,
    paymasterContext: { policyId: paymasterPolicyId },
    userOperation: {
      // ZeroDev's default fetcher calls `zd_getUserOperationGasPrice`,
      // which Alchemy's bundler rejects. Use Alchemy's
      // `rundler_maxPriorityFeePerGas` instead and combine with viem's
      // base-fee estimate for maxFeePerGas. Falls back to a static
      // floor (0.001 gwei) if the rundler method isn't available.
      estimateFeesPerGas: async () => {
        const STATIC_MIN_PRIORITY = 1_000_000n;
        let maxPriorityFeePerGas = STATIC_MIN_PRIORITY;
        try {
          const resp = await fetch(bundlerUrl, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              jsonrpc: "2.0",
              id: 1,
              method: "rundler_maxPriorityFeePerGas",
              params: [],
            }),
          });
          const json = await resp.json();
          if (typeof json.result === "string") {
            const rundlerFee = BigInt(json.result);
            maxPriorityFeePerGas =
              rundlerFee > STATIC_MIN_PRIORITY
                ? rundlerFee
                : STATIC_MIN_PRIORITY;
          }
        } catch {
          /* fall through to static floor */
        }
        const baseFees = await publicClient.estimateFeesPerGas();
        const maxFeePerGas = baseFees.maxFeePerGas + maxPriorityFeePerGas;
        return { maxFeePerGas, maxPriorityFeePerGas };
      },
    },
  });

  return { client, account, accountAddress: account.address };
}
