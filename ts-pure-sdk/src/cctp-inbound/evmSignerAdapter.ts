/**
 * Bridge `EvmSigner` → a viem `LocalAccount` that ZeroDev's Kernel
 * ECDSA validator accepts as an `owner`.
 *
 * Kernel's validator calls:
 *   - `signMessage({ message: { raw } })` to sign the UserOp hash.
 *   - `signTypedData({ ... })` to sign the Permit2 witness (and any
 *     other EIP-712 message routed through the smart account).
 *
 * `EvmSigner.signTypedData` is required on all callers. `signMessage`
 * is optional on `EvmSigner` — this adapter throws a clear error if
 * the CCTP path is reached with an `EvmSigner` that lacks it.
 *
 * `signTransaction` is provided as a stub: Kernel never routes txs
 * through the owner account (the bundler submits UserOps), so calling
 * it would be a programmer error.
 */

import type { Hex, LocalAccount } from "viem";
import { toAccount } from "viem/accounts";
import type { EvmSigner } from "../evm/wallet.js";

/**
 * Wrap an `EvmSigner` in a viem `LocalAccount` so ZeroDev Kernel's
 * `signerToEcdsaValidator` can use it as the account owner.
 */
export function evmSignerToKernelOwner(signer: EvmSigner): LocalAccount {
  return toAccount({
    address: signer.address as Hex,
    async signMessage({ message }) {
      if (!signer.signMessage) {
        throw new Error(
          "CCTP-inbound flow requires `EvmSigner.signMessage`. Extend your signer — e.g. `(m) => walletClient.signMessage({ account, message: m })`.",
        );
      }
      // viem's `message` is `SignableMessage`: either a string or
      // `{ raw: Hex }`. Kernel always passes `{ raw }` — but be
      // defensive about the string shape too.
      const raw =
        typeof message === "string"
          ? (new TextEncoder().encode(message) as unknown as string)
          : (message.raw as string);
      return (await signer.signMessage({ raw })) as Hex;
    },
    async signTypedData(typedData) {
      // Narrow viem's broad SignTypedDataParameters to the SDK's
      // internal EIP712TypedData shape — at runtime they're
      // structurally identical, the difference is just generic-heavy
      // typing on viem's side.
      return (await signer.signTypedData(
        typedData as unknown as Parameters<EvmSigner["signTypedData"]>[0],
      )) as Hex;
    },
    async signTransaction() {
      throw new Error(
        "signTransaction is not supported on the Kernel owner — UserOps are submitted via the bundler, not from the owner EOA directly.",
      );
    },
  });
}
