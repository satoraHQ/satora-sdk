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

/**
 * The recipient amounts a claim of `funded` sats pays: each commitment plus a
 * share of the surplus in proportion to it, rounding leftovers one sat each in
 * list order. `fee` defaults to the committed fee; a claim that raises its fee
 * to meet a fee rate pays from the surplus, so pass the actual fee to match it.
 * Throws if the funding cannot cover the commitments and fee.
 */
export function allocateBitcoinPayout(
  payout: BitcoinPayout,
  funded: bigint,
  fee: bigint = BigInt(payout.feeSats),
): BitcoinPayout {
  const amounts = payout.recipients.map((r) => BigInt(r.amountSats));
  const total = amounts.reduce((a, b) => a + b, 0n);
  const remainder = funded - total - fee;
  if (remainder < 0n)
    throw new Error(
      "Funded amount cannot cover the committed recipients and fee",
    );
  const shares = amounts.map((amount) => (remainder * amount) / total);
  let leftover = remainder - shares.reduce((a, b) => a + b, 0n);
  return {
    feeSats: fee.toString(),
    recipients: payout.recipients.map((recipient, index) => {
      const extra = leftover > 0n ? 1n : 0n;
      leftover -= extra;
      return {
        address: recipient.address,
        amountSats: (amounts[index] + shares[index] + extra).toString(),
      };
    }),
  };
}
