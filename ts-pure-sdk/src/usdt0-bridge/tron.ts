import { sha256 } from "@noble/hashes/sha2.js";
import { base58check } from "@scure/base";

/** Decode and validate the Base58Check recipient before it reaches a signing flow. */
export function tronAddressToBytes32(address: string): `0x${string}` {
  if (address.length !== 34) throw new Error("Invalid Tron address length");
  const decoded = base58check(sha256).decode(address);
  if (decoded.length !== 21 || decoded[0] !== 0x41) {
    throw new Error("Invalid Tron address prefix");
  }
  const account = decoded.slice(1);
  if (account.every((b) => b === 0))
    throw new Error("Zero Tron recipient is not allowed");
  return `0x${Array.from(account, (b) => b.toString(16).padStart(2, "0"))
    .join("")
    .padStart(64, "0")}`;
}

export function isValidTronAddress(address: string): boolean {
  try {
    tronAddressToBytes32(address);
    return true;
  } catch {
    return false;
  }
}
