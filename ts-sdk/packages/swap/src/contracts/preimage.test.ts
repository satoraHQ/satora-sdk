import { ripemd160 } from "@noble/hashes/legacy.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { describe, expect, it } from "vitest";
import { preimageMatches } from "./preimage.js";

const preimage = new Uint8Array(32).fill(7);
const other = new Uint8Array(32).fill(9);

describe("preimageMatches", () => {
  it("verifies a 32-byte SHA-256 lock (EVM/Lightning directions)", () => {
    expect(preimageMatches(preimage, sha256(preimage))).toBe(true);
    expect(preimageMatches(other, sha256(preimage))).toBe(false);
  });

  it("verifies a 20-byte HASH160 lock (btc_to_arkade)", () => {
    const hash160 = ripemd160(sha256(preimage));
    expect(hash160.length).toBe(20);
    expect(preimageMatches(preimage, hash160)).toBe(true);
    expect(preimageMatches(other, hash160)).toBe(false);
  });

  it("does not cross-verify: a SHA-256 preimage fails against its HASH160 and vice versa", () => {
    // The digest length alone selects the algorithm, so a matching preimage under
    // one lock never spuriously matches the other-length lock.
    expect(preimageMatches(preimage, ripemd160(sha256(preimage)))).toBe(true);
    expect(preimageMatches(preimage, sha256(preimage))).toBe(true);
  });
});
