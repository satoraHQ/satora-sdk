/**
 * Solana-as-a-CCTP-destination plumbing.
 *
 * Solana isn't an EVM chain, so a few of the SDK's chain helpers get a
 * dedicated branch (`isBridgeOnlyChain`, `toChain`, `getCctpBridgeTokens`).
 * These tests pin those branches so a refactor that loses the Solana path
 * gets caught at CI rather than at swap time.
 */

import { describe, expect, it } from "vitest";
import {
  CCTP_DOMAINS,
  getCctpBridgeTokens,
  isBridgeOnlyChain,
  isSolanaToken,
  isValidSolanaAddress,
  solanaAddressToBytes32,
  toChain,
  USDC_ADDRESSES,
} from "../src/index.js";

const SOL_USDC_MINT = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

describe("Solana CCTP plumbing", () => {
  it("CCTP_DOMAINS contains Solana with domain 5", () => {
    expect(CCTP_DOMAINS.Solana).toBe(5);
  });

  it("USDC_ADDRESSES exposes the Solana SPL mint", () => {
    expect(USDC_ADDRESSES.Solana).toBe(SOL_USDC_MINT);
  });

  it("isSolanaToken matches the chain string case-insensitively", () => {
    expect(isSolanaToken("Solana")).toBe(true);
    expect(isSolanaToken("solana")).toBe(true);
    expect(isSolanaToken("SOLANA")).toBe(true);
    expect(isSolanaToken("Bitcoin")).toBe(false);
    expect(isSolanaToken("42161")).toBe(false);
  });

  it("isBridgeOnlyChain treats Solana as bridge-only", () => {
    expect(isBridgeOnlyChain("Solana")).toBe(true);
    // Source EVM chains stay non-bridge-only.
    expect(isBridgeOnlyChain("42161")).toBe(false);
    expect(isBridgeOnlyChain("1")).toBe(false);
  });

  it("toChain normalizes 'solana' to 'Solana'", () => {
    expect(toChain("solana")).toBe("Solana");
    expect(toChain("Solana")).toBe("Solana");
  });

  it("getCctpBridgeTokens emits a USDC-on-Solana entry", () => {
    const tokens = getCctpBridgeTokens();
    const solanaUsdc = tokens.find((t) => t.chain === "Solana");
    expect(solanaUsdc).toBeDefined();
    expect(solanaUsdc?.symbol).toBe("USDC");
    expect(solanaUsdc?.token_id).toBe(SOL_USDC_MINT);
    expect(solanaUsdc?.decimals).toBe(6);
  });
});

describe("Solana address helpers", () => {
  it("isValidSolanaAddress accepts canonical pubkeys", () => {
    expect(isValidSolanaAddress(SOL_USDC_MINT)).toBe(true);
    // System program — all zero bytes, base58-encoded.
    expect(isValidSolanaAddress("11111111111111111111111111111111")).toBe(true);
  });

  it("isValidSolanaAddress rejects junk", () => {
    expect(isValidSolanaAddress("")).toBe(false);
    expect(isValidSolanaAddress("not_base58_!!")).toBe(false);
    // EVM hex address — base58 charset overlaps but length is wrong.
    expect(
      isValidSolanaAddress("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
    ).toBe(false);
    // Truncated pubkey decodes to fewer than 32 bytes.
    expect(isValidSolanaAddress("1111")).toBe(false);
  });

  it("solanaAddressToBytes32 returns 0x-prefixed 32-byte hex", () => {
    const hex = solanaAddressToBytes32(SOL_USDC_MINT);
    expect(hex).toMatch(/^0x[0-9a-f]{64}$/);
    // Round-trip: the upper bytes are non-zero (unlike a left-padded EVM
    // address, which would be all zero in the upper 12 bytes). This is the
    // structural difference the backend's `address_to_bytes32` cannot
    // produce, and exactly the case we need CCTP to accept verbatim.
    const upperZero = hex
      .slice(2, 26)
      .split("")
      .every((c) => c === "0");
    expect(upperZero).toBe(false);
  });

  it("solanaAddressToBytes32 rejects non-32-byte decodes", () => {
    expect(() => solanaAddressToBytes32("1111")).toThrow();
  });
});
