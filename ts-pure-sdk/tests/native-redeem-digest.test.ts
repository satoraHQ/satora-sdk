import { describe, expect, it } from "vitest";
import {
  buildNativeRedeemDigest,
  computeCoordinatorCallsHash,
  deriveEvmAddress,
  NATIVE_TOKEN_ADDRESS,
  signEvmDigest,
} from "../src/evm/index.js";

/**
 * The vector `contracts/test/HTLCNativeDomain.t.sol::test_offChainSignatureVector`
 * pins and settles on chain, and the server checks in
 * `swap/src/api/claim_gasless.rs::native_digest_tests`: same domain, digest
 * and signer here.
 */
describe("buildNativeRedeemDigest", () => {
  const vector = {
    htlcAddress: "0x1000000000000000000000000000000000000001",
    chainId: 30,
    preimage:
      "0x1111111111111111111111111111111111111111111111111111111111111111",
    amount: 100_000_000n,
    sender: "0x3333333333333333333333333333333333333333",
    timelock: 1_800_000_000,
    caller: "0x5555555555555555555555555555555555555555",
    destination: "0x7564105E977516C53bE337314c7E53838967bDaC",
    sweepToken: NATIVE_TOKEN_ADDRESS,
    minAmountOut: 0n,
    callsHash:
      "0x0000000000000000000000000000000000000000000000000000000000000000",
    htlcVersion: 1,
  };

  it("matches the contract vector", () => {
    expect(buildNativeRedeemDigest(vector)).toBe(
      "0x1397895b4d20525c9ecb07e36978478ad900e12dec0c63474100b53407416c09",
    );
  });

  it("differs from the ERC-20 domain for the same inputs", () => {
    // Version "4" is HTLCErc20's; a native deployment starts at 1.
    expect(buildNativeRedeemDigest({ ...vector, htlcVersion: undefined })).toBe(
      buildNativeRedeemDigest(vector),
    );
    expect(buildNativeRedeemDigest({ ...vector, chainId: 31 })).not.toBe(
      buildNativeRedeemDigest(vector),
    );
  });

  it("signs with the derived claim key so the relay recovers the claimant", () => {
    // The vector's claimant: `vm.addr(0x44…44)` in the contract test.
    const key = new Uint8Array(32).fill(0x44);
    expect(deriveEvmAddress(key).toLowerCase()).toBe(
      vector.destination.toLowerCase(),
    );
    const sig = signEvmDigest(key, buildNativeRedeemDigest(vector));
    expect(sig.v).toBe(27);
    expect(sig.r).toBe(
      "0xc4c95466427c3cb06f1fe5787ba79e0d6fca9a3f0b63db8f0fe1778c23c233e6",
    );
    expect(sig.s).toBe(
      "0x1dc3d44c7270f846d8093cac1c2641c51ed02c3e26e546ab49c383a21223fbf1",
    );
  });

  it("uses the same empty-calls hash the native coordinator computes", () => {
    // abi.encode(Call[]) of no calls: the offset word then a zero length.
    expect(computeCoordinatorCallsHash([])).toBe(
      "0x569e75fc77c1a856f6daaf9e69d8a9566ca34aa47f9133711ce065a571af0cfd",
    );
  });
});
