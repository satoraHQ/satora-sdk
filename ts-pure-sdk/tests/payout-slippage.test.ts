import { secp256k1 } from "@noble/curves/secp256k1.js";
import { hex } from "@scure/base";
import * as btc from "@scure/btc-signer";
import { describe, expect, it } from "vitest";
import {
  bitcoinNetworkOf,
  commitBitcoinPayment,
  minimumBitcoinPayout,
} from "../src/create/payout-slippage.js";
import {
  buildOnchainClaimTransaction,
  computeHash160,
} from "../src/refund/onchain.js";

const key = new Uint8Array(32).fill(2);
const preimage = new Uint8Array(32).fill(42);
const a = btc.p2wpkh(secp256k1.getPublicKey(key), btc.TEST_NETWORK).address;
const b = btc.p2wpkh(
  secp256k1.getPublicKey(new Uint8Array(32).fill(3)),
  btc.TEST_NETWORK,
).address;
if (!a || !b) throw new Error("fixture addresses");
const request = {
  recipients: [
    { address: a, amountSats: "1051" },
    { address: b, amountSats: "2052" },
  ],
  feeSats: "100",
  maxSlippageBps: 100,
};
const claim = (htlcAmount: bigint, bitcoinPayout: unknown) =>
  btc.Transaction.fromRaw(
    hex.decode(
      buildOnchainClaimTransaction({
        fundingTxId: "00".repeat(32),
        fundingVout: 0,
        htlcAmount,
        hashLock: hex.encode(computeHash160(preimage)),
        userClaimPubKey: hex.encode(secp256k1.getPublicKey(key).slice(1)),
        serverRefundPubKey: hex.encode(
          secp256k1.getPublicKey(new Uint8Array(32).fill(1)).slice(1),
        ),
        userSecretKey: hex.encode(key),
        preimage: hex.encode(preimage),
        refundLocktime: 1700000000,
        destinationAddress: a,
        feeRateSatPerVb: 0.1,
        network: "signet",
        bitcoinPayout: bitcoinPayout as never,
      }).txHex,
    ),
  );

describe("payment commitments", () => {
  it("commits each recipient to its floor and the server to floors plus fee", () => {
    const { payout, minimumPayoutSats, nominalSats } = commitBitcoinPayment(
      request,
      btc.TEST_NETWORK,
    );
    expect(payout.recipients.map((r) => r.amountSats)).toEqual([
      "1041",
      "2032",
    ]);
    expect(payout.feeSats).toBe("100");
    expect(minimumPayoutSats).toBe(3173n);
    expect(nominalSats).toBe(3203n);
    expect(minimumBitcoinPayout(request, btc.TEST_NETWORK)).toBe(3173n);
    expect(request.recipients[0].amountSats).toBe("1051");
  });
  it("pays at least the floors and conserves every sat for any funding at or above the minimum", () => {
    const { payout } = commitBitcoinPayment(request, btc.TEST_NETWORK);
    for (let funded = 3173n; funded <= 3240n; funded++) {
      const tx = claim(funded, payout);
      const [x, y] = [0, 1].map((i) => tx.getOutput(i).amount ?? 0n);
      expect(x + y + 100n).toBe(funded);
      expect(x).toBeGreaterThanOrEqual(1041n);
      expect(y).toBeGreaterThanOrEqual(2032n);
    }
  });
  it("pays about the nominal amounts when funded at the nominal total", () => {
    const { payout } = commitBitcoinPayment(request, btc.TEST_NETWORK);
    const tx = claim(3203n, payout);
    const [x, y] = [0, 1].map((i) => tx.getOutput(i).amount ?? 0n);
    expect(x - 1051n).toBeGreaterThanOrEqual(-1n);
    expect(x - 1051n).toBeLessThanOrEqual(1n);
    expect(y - 2052n).toBeGreaterThanOrEqual(-1n);
    expect(y - 2052n).toBeLessThanOrEqual(1n);
  });
  it("defaults to no tolerance and rejects invalid ones", () => {
    const { payout } = commitBitcoinPayment(
      { ...request, maxSlippageBps: undefined },
      btc.TEST_NETWORK,
    );
    expect(payout.recipients.map((r) => r.amountSats)).toEqual([
      "1051",
      "2052",
    ]);
    for (const maxSlippageBps of [-1, 101, 0.5, Number.NaN])
      expect(() =>
        commitBitcoinPayment({ ...request, maxSlippageBps }, btc.TEST_NETWORK),
      ).toThrow("basis points");
  });
  it("never commits a floor below dust", () => {
    const { payout } = commitBitcoinPayment(
      { ...request, recipients: [{ address: a, amountSats: "550" }] },
      btc.TEST_NETWORK,
    );
    expect(payout.recipients[0].amountSats).toBe("546");
    expect(() =>
      commitBitcoinPayment(
        { ...request, recipients: [{ address: a, amountSats: "545" }] },
        btc.TEST_NETWORK,
      ),
    ).toThrow("at least 546 sats");
  });
  it("checks recipients against the recovery address network", () => {
    expect(bitcoinNetworkOf(a)).toBe(btc.TEST_NETWORK);
    const mainnet = btc.p2wpkh(secp256k1.getPublicKey(key)).address;
    if (!mainnet) throw new Error("fixture address");
    expect(() =>
      commitBitcoinPayment(
        { ...request, recipients: [{ address: mainnet, amountSats: "1000" }] },
        btc.TEST_NETWORK,
      ),
    ).toThrow("not a valid address for this network");
    expect(() => bitcoinNetworkOf("not-an-address")).toThrow(
      "Invalid Bitcoin recovery address",
    );
  });
});
