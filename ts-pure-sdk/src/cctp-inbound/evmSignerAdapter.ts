/**
 * Bridge `EvmSigner` → a viem `LocalAccount` that ZeroDev's Kernel
 * (under EIP-7702) accepts as the EOA being delegated.
 *
 * Kernel calls:
 *   - `signMessage({ message: { raw } })` to sign the UserOp hash.
 *   - `signTypedData({ ... })` to sign the Permit2 witness (and any
 *     other EIP-712 message routed through the account).
 *   - `signAuthorization({...})` to sign the 7702 auth tuple on the
 *     first UserOp from this EOA — installs the delegation on-chain.
 *
 * `EvmSigner.signTypedData` is required on all callers. `signMessage`
 * and `signAuthorization` are optional on `EvmSigner` — this adapter
 * throws a clear error if the CCTP path is reached with an
 * `EvmSigner` that lacks them.
 *
 * `signTransaction` is provided as a stub: Kernel never routes txs
 * through the owner account (the bundler submits UserOps), so calling
 * it would be a programmer error.
 */

import type { Hex, LocalAccount } from "viem";
import { toAccount } from "viem/accounts";
import type { EvmSigner } from "../evm/wallet.js";

/**
 * Wrap an `EvmSigner` in a viem `LocalAccount` so ZeroDev Kernel can
 * use it as the EIP-7702 delegated account (`eip7702Account`).
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
    async signAuthorization(authorization) {
      if (!signer.signAuthorization) {
        throw new Error(
          "CCTP-inbound flow under EIP-7702 requires `EvmSigner.signAuthorization`. Extend your signer — e.g. `(a) => walletClient.signAuthorization({ account, ...a })`.",
        );
      }
      // viem's `AuthorizationRequest` is a `OneOf` — the delegation
      // target arrives as EITHER `address` (the canonical field, which
      // Kernel/viem actually use) OR the `contractAddress` alias.
      // Reading only `contractAddress` yields `undefined` on the
      // `address` branch, which then blows up inside viem's
      // `hashAuthorization` ("Cannot read properties of undefined").
      const contractAddress = (authorization.address ??
        authorization.contractAddress) as string;
      const result = await signer.signAuthorization({
        chainId: authorization.chainId as number,
        contractAddress,
        nonce: authorization.nonce as number,
      });
      // viem accepts either `v` or `yParity`; the field they actually
      // read is `yParity`. Pass both for compatibility.
      return {
        r: result.r as Hex,
        s: result.s as Hex,
        v: result.v !== undefined ? BigInt(result.v) : undefined,
        yParity: result.yParity,
        chainId: result.chainId,
        address: result.address as Hex,
        nonce: result.nonce,
      };
    },
    async signTransaction() {
      throw new Error(
        "signTransaction is not supported on the Kernel owner — UserOps are submitted via the bundler, not from the owner EOA directly.",
      );
    },
  });
}
