import * as btc from "@scure/btc-signer";
import type { BitcoinPayout } from "../storage/types.js";

const MAX_MONEY = 2_100_000_000_000_000n;
/** Upper bound for a payout claim fee, committed or raised to meet a fee rate. */
export const MAX_PAYOUT_FEE_SATS = 100_000n;

/** Validate exact recipient commitments before signing. Duplicate addresses stay separate. */
export function validateBitcoinPayout(
  payout: BitcoinPayout,
  network: typeof btc.NETWORK,
): { total: bigint; fee: bigint } {
  if (typeof payout !== "object" || payout === null)
    throw new Error("A Bitcoin payout must be an object");
  if (
    !Array.isArray(payout.recipients) ||
    payout.recipients.length < 1 ||
    payout.recipients.length > 100
  )
    throw new Error("A Bitcoin payout needs 1–100 recipients");
  const sats = (value: string): bigint => {
    if (typeof value !== "string" || !/^[0-9]{1,16}$/.test(value))
      throw new Error("Payout amounts must be integer satoshi strings");
    const amount = BigInt(value);
    if (amount > MAX_MONEY) throw new Error("Payout exceeds Bitcoin supply");
    return amount;
  };
  const fee = sats(payout.feeSats);
  if (fee < 1n || fee > MAX_PAYOUT_FEE_SATS)
    throw new Error("Payout fee must be between 1 and 100,000 sats");
  let total = 0n;
  for (const recipient of payout.recipients) {
    let decoded: ReturnType<ReturnType<typeof btc.Address>["decode"]>;
    try {
      decoded = btc.Address(network).decode(recipient.address);
    } catch {
      throw new Error(
        "Recipient address is not a valid address for this network",
      );
    }
    // A conservative standard-output dust floor. Never accept arbitrary scripts.
    if (!decoded || !["pkh", "sh", "wpkh", "wsh", "tr"].includes(decoded.type))
      throw new Error("Unsupported Bitcoin recipient address");
    const amount = sats(recipient.amountSats);
    if (amount < 546n)
      throw new Error("Each recipient must receive at least 546 sats");
    total += amount;
  }
  if (total + fee > MAX_MONEY) throw new Error("Payout exceeds Bitcoin supply");
  return { total, fee };
}
