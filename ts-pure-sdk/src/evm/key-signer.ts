/**
 * An {@link EvmSigner} backed by one of the SDK's own derived keys, for the
 * paths where that key owns a sponsored Kernel account (EIP-7702) rather than
 * sending transactions itself: the UserOp claim, and the CCTP-inbound
 * settlement when the swap was created gasless.
 */
import { type Hex, toHex } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import type { EvmSigner } from "./wallet.js";

/**
 * Wrap a derived private key as the Kernel-owner {@link EvmSigner}:
 * `signMessage` (UserOp hash), `signTypedData` (Permit2 witness) and
 * `signAuthorization` (7702 tuple). The key never sends transactions on these
 * paths, so the transaction methods throw.
 */
export function keyToKernelOwnerSigner(
  secretKey: Uint8Array,
  chainId: number,
): EvmSigner {
  const account = privateKeyToAccount(toHex(secretKey));
  const unsupported = (m: string) => async (): Promise<never> => {
    throw new Error(
      `keyToKernelOwnerSigner: ${m} is not supported on the sponsored UserOp path`,
    );
  };
  return {
    address: account.address,
    chainId,
    signMessage: ({ raw }) =>
      account.signMessage({ message: { raw: raw as Hex } }),
    signTypedData: (typedData) =>
      account.signTypedData(
        typedData as unknown as Parameters<typeof account.signTypedData>[0],
      ),
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
    sendTransaction: unsupported("sendTransaction"),
    waitForReceipt: unsupported("waitForReceipt"),
    getTransaction: unsupported("getTransaction"),
    call: unsupported("call"),
  };
}
