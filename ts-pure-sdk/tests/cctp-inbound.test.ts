import { describe, expect, it } from "vitest";
import { deriveEvmAddress } from "../src/evm/signing.js";
import {
  addressToBytes32,
  buildDomainSeparator,
  encodeDepositForBurn,
  MAX_UINT256,
  signEip2612Permit,
  signPermit2Witness,
  USDC_DOMAIN_FIELDS,
} from "../src/index.js";

// Deterministic test key — keccak/noble behavior is the same everywhere so
// fixed inputs → fixed outputs regardless of environment.
const TEST_KEY = new Uint8Array(32).fill(0x42);
const TEST_OWNER = deriveEvmAddress(TEST_KEY);

describe("cctp-inbound · encodeDepositForBurn", () => {
  const BASE_PARAMS = {
    amount: 10_000_000n, // 10 USDC
    destinationDomain: 3, // Arbitrum
    mintRecipient: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    burnToken: "0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85", // Optimism USDC
    maxFee: 500n,
  };

  it("produces a 228-byte calldata (4-byte selector + 7 × 32-byte args)", () => {
    const data = encodeDepositForBurn(BASE_PARAMS);
    // 0x + 4*2 selector + 7*64 args = 2 + 8 + 448 = 458 chars
    expect(data).toMatch(/^0x[0-9a-f]{456}$/);
  });

  it("is deterministic for identical inputs", () => {
    expect(encodeDepositForBurn(BASE_PARAMS)).toBe(
      encodeDepositForBurn(BASE_PARAMS),
    );
  });

  it("zero-pads a 20-byte mintRecipient to bytes32", () => {
    const data = encodeDepositForBurn(BASE_PARAMS);
    // mintRecipient is arg #3, lives at offset: 0x + 8(selector) + 2*64(amount,domain) = 10+128 = 138
    const mintRecipient = data.slice(138, 138 + 64);
    // First 12 bytes (24 hex) must be zero, last 20 bytes (40 hex) is the address
    expect(mintRecipient.slice(0, 24)).toBe("0".repeat(24));
    expect(mintRecipient.slice(24)).toBe(
      "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
  });

  it("accepts a pre-padded bytes32 mintRecipient", () => {
    const padded = addressToBytes32(BASE_PARAMS.mintRecipient);
    const data = encodeDepositForBurn({
      ...BASE_PARAMS,
      mintRecipient: padded,
    });
    expect(data).toBe(encodeDepositForBurn(BASE_PARAMS));
  });

  it("defaults destinationCaller to bytes32(0) and minFinalityThreshold to 1000", () => {
    const data = encodeDepositForBurn(BASE_PARAMS);
    // destinationCaller is arg #5, lives after selector(8) + 4*64 = offset 10+256 = 266
    const destinationCaller = data.slice(266, 266 + 64);
    expect(destinationCaller).toBe("0".repeat(64));
    // minFinalityThreshold is arg #7 (last), offset = 10 + 6*64 = 394
    const finality = data.slice(394, 394 + 64);
    expect(BigInt(`0x${finality}`)).toBe(1000n);
  });

  it("encodes amount + maxFee as uint256", () => {
    const data = encodeDepositForBurn({
      ...BASE_PARAMS,
      amount: 2n ** 64n,
      maxFee: 2n ** 32n,
    });
    // amount is arg #1 (offset 10)
    expect(BigInt(`0x${data.slice(10, 74)}`)).toBe(2n ** 64n);
    // maxFee is arg #6 (offset 10 + 5*64 = 330)
    expect(BigInt(`0x${data.slice(330, 394)}`)).toBe(2n ** 32n);
  });

  it("rejects invalid mintRecipient length", () => {
    expect(() =>
      encodeDepositForBurn({ ...BASE_PARAMS, mintRecipient: "0xdeadbeef" }),
    ).toThrow(/Invalid mintRecipient/);
  });
});

describe("cctp-inbound · signEip2612Permit", () => {
  const DOMAIN = buildDomainSeparator({
    name: USDC_DOMAIN_FIELDS[42161].name,
    version: USDC_DOMAIN_FIELDS[42161].version,
    chainId: 42161,
    verifyingContract: USDC_DOMAIN_FIELDS[42161].address,
  });

  const BASE_PARAMS = {
    secretKey: TEST_KEY,
    domainSeparator: DOMAIN,
    owner: TEST_OWNER,
    value: MAX_UINT256,
    nonce: 0,
    deadline: 1_900_000_000n,
  };

  it("returns v, r, s with correct hex shapes", () => {
    const sig = signEip2612Permit(BASE_PARAMS);
    expect(sig.v === 27 || sig.v === 28).toBe(true);
    expect(sig.r).toMatch(/^0x[0-9a-f]{64}$/);
    expect(sig.s).toMatch(/^0x[0-9a-f]{64}$/);
    expect(sig.value).toBe(MAX_UINT256.toString());
    expect(sig.deadline).toBe(1_900_000_000);
  });

  it("is deterministic for identical inputs", () => {
    const a = signEip2612Permit(BASE_PARAMS);
    const b = signEip2612Permit(BASE_PARAMS);
    expect(a).toEqual(b);
  });

  it("produces a different signature when nonce changes", () => {
    const a = signEip2612Permit(BASE_PARAMS);
    const b = signEip2612Permit({ ...BASE_PARAMS, nonce: 1 });
    expect(a.r).not.toBe(b.r);
  });

  it("defaults spender to Permit2", () => {
    const withExplicit = signEip2612Permit({
      ...BASE_PARAMS,
      spender: "0x000000000022D473030F116dDEE9F6B43aC78BA3",
    });
    const withDefault = signEip2612Permit(BASE_PARAMS);
    expect(withExplicit).toEqual(withDefault);
  });
});

describe("cctp-inbound · signPermit2Witness", () => {
  const FUNDING = {
    chainId: 42161,
    coordinatorAddress: "0x1111111111111111111111111111111111111111",
    sourceToken: USDC_DOMAIN_FIELDS[42161].address,
    sourceAmount: 10_000_000n,
    preimageHash:
      "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
    lockToken: "0x2222222222222222222222222222222222222222",
    claimAddress: TEST_OWNER,
    refundAddress: "0x1111111111111111111111111111111111111111",
    timelock: 1_900_000_000,
    callsHash:
      "0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd",
    nonce: 0x1234n,
    deadline: 1_900_000_000n,
  };

  it("returns a 65-byte compact signature (r || s || v)", () => {
    const sig = signPermit2Witness({
      secretKey: TEST_KEY,
      funding: FUNDING,
    });
    // 0x + 65*2 = 132 chars
    expect(sig.signature).toMatch(/^0x[0-9a-f]{130}$/);
    expect(sig.nonce).toBe("4660"); // 0x1234
    expect(sig.deadline).toBe(1_900_000_000);
  });

  it("v byte is 27 or 28", () => {
    const sig = signPermit2Witness({
      secretKey: TEST_KEY,
      funding: FUNDING,
    });
    const v = parseInt(sig.signature.slice(-2), 16);
    expect(v === 27 || v === 28).toBe(true);
  });

  it("is deterministic for identical inputs", () => {
    const a = signPermit2Witness({ secretKey: TEST_KEY, funding: FUNDING });
    const b = signPermit2Witness({ secretKey: TEST_KEY, funding: FUNDING });
    expect(a).toEqual(b);
  });

  it("produces a different signature when the witness changes", () => {
    const a = signPermit2Witness({ secretKey: TEST_KEY, funding: FUNDING });
    const b = signPermit2Witness({
      secretKey: TEST_KEY,
      funding: { ...FUNDING, sourceAmount: 20_000_000n },
    });
    expect(a.signature).not.toBe(b.signature);
  });
});
