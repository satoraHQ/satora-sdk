import * as btc from "@scure/btc-signer";
import type { BitcoinPayout } from "../storage/types.js";
import { validateBitcoinPayout } from "./bitcoin-payout.js";

/** A requested payment: nominal recipient amounts, the claim fee and how far below them DEX execution may land. */
export interface BitcoinPaymentRequest {
  recipients: { address: string; amountSats: string }[];
  feeSats: string;
  /** Maximum reduction per recipient, in basis points (0–100). Default 0. */
  maxSlippageBps?: number;
}

const DUST = 546n;

function tolerance(bps: number | undefined): bigint {
  const value = bps ?? 0;
  if (!Number.isInteger(value) || value < 0 || value > 100)
    throw new Error("Payment slippage must be between 0 and 100 basis points");
  return BigInt(value);
}

/** Lowest amount a recipient may receive: nominal less the tolerance, never below dust. */
function floorOf(amount: bigint, bps: bigint): bigint {
  const loss = (amount * bps) / 10000n;
  return amount - (loss < amount - DUST ? loss : amount - DUST);
}

const NETWORKS = [
  btc.NETWORK,
  btc.TEST_NETWORK,
  { ...btc.TEST_NETWORK, bech32: "bcrt" },
];

/** The network of a Bitcoin address, so recipients can be checked against the recovery address. */
export function bitcoinNetworkOf(address: string): typeof btc.NETWORK {
  const network = NETWORKS.find((candidate) => {
    try {
      btc.Address(candidate).decode(address);
      return true;
    } catch {
      return false;
    }
  });
  if (!network) throw new Error("Invalid Bitcoin recovery address");
  return network;
}

/**
 * Turn a payment request into the recipient commitments a claim enforces.
 *
 * Each recipient commits to its floor (nominal less the tolerance). The claim
 * splits any surplus in proportion to the floors, so a payment funded at its
 * nominal total pays roughly the nominal amounts. `minimumPayoutSats` is the
 * floor total plus the fee: the least the server may fund the HTLC with.
 */
export function commitBitcoinPayment(
  request: BitcoinPaymentRequest,
  network: typeof btc.NETWORK,
): { payout: BitcoinPayout; minimumPayoutSats: bigint; nominalSats: bigint } {
  const bps = tolerance(request.maxSlippageBps);
  const nominal: BitcoinPayout = {
    recipients: request.recipients.map((r) => ({ ...r })),
    feeSats: request.feeSats,
  };
  const { total, fee } = validateBitcoinPayout(nominal, network);
  const payout: BitcoinPayout = {
    recipients: nominal.recipients.map((r) => ({
      address: r.address,
      amountSats: floorOf(BigInt(r.amountSats), bps).toString(),
    })),
    feeSats: nominal.feeSats,
  };
  const floors = validateBitcoinPayout(payout, network);
  return {
    payout,
    minimumPayoutSats: floors.total + floors.fee,
    nominalSats: total + fee,
  };
}

/** Minimum HTLC funding for a payment request: floors plus the fee. */
export function minimumBitcoinPayout(
  request: BitcoinPaymentRequest,
  network: typeof btc.NETWORK,
): bigint {
  return commitBitcoinPayment(request, network).minimumPayoutSats;
}
