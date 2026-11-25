import { secp256k1 } from "@noble/curves/secp256k1.js";
import { hex } from "@scure/base";
import * as btc from "@scure/btc-signer";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Client } from "../src/client.js";
import * as electrum from "../src/electrum-ws.js";
import * as esplora from "../src/esplora.js";
import { InMemorySwapStorage } from "../src/storage/index.js";

afterEach(() => vi.restoreAllMocks());

import {
  buildOnchainClaimTransaction,
  computeHash160,
} from "../src/refund/onchain.js";
import { SqliteSwapStorage } from "../src/storage/sqlite.js";
import { storedSwap } from "./evm-funding-fixture.js";

const key = new Uint8Array(32).fill(2);
const preimage = new Uint8Array(32).fill(42);
const address = btc.p2wpkh(secp256k1.getPublicKey(key), btc.TEST_NETWORK)
  .address!;
const second = btc.p2wpkh(
  secp256k1.getPublicKey(new Uint8Array(32).fill(3)),
  btc.TEST_NETWORK,
).address!;
const payout = {
  recipients: [
    { address, amountSats: "30000" },
    { address: second, amountSats: "68000" },
  ],
  feeSats: "500",
};
const params = {
  fundingTxId: "00".repeat(32),
  fundingVout: 0,
  htlcAmount: 100000n,
  hashLock: hex.encode(computeHash160(preimage)),
  userClaimPubKey: hex.encode(secp256k1.getPublicKey(key).slice(1)),
  serverRefundPubKey: hex.encode(
    secp256k1.getPublicKey(new Uint8Array(32).fill(1)).slice(1),
  ),
  userSecretKey: hex.encode(key),
  preimage: hex.encode(preimage),
  refundLocktime: 1700000000,
  destinationAddress: address,
  feeRateSatPerVb: 1,
  network: "signet" as const,
  bitcoinPayout: payout,
};
const decode = (result: ReturnType<typeof buildOnchainClaimTransaction>) =>
  btc.Transaction.fromRaw(hex.decode(result.txHex));

describe("multi-recipient Bitcoin claim primitive", () => {
  it("preserves recipient order and splits surplus equally", () => {
    const result = buildOnchainClaimTransaction(params);
    const tx = decode(result);
    expect(tx.outputsLength).toBe(2);
    expect([0, 1].map((i) => tx.getOutput(i).amount)).toEqual([30750n, 68750n]);
    expect(tx.getOutput(0).script).toEqual(
      btc.p2wpkh(secp256k1.getPublicKey(key), btc.TEST_NETWORK).script,
    );
    expect(tx.getOutput(1).script).not.toEqual(tx.getOutput(0).script);
    expect(tx.getInput(0).sequence).toBe(0xfffffffd);
    expect(result.vsize).toBe(tx.vsize);
  });
  it("bumps fees from surplus without going below recipient commitments", () => {
    const tx = decode(
      buildOnchainClaimTransaction({
        ...params,
        bitcoinPayout: { ...payout, feeSats: "1000" },
      }),
    );
    expect([0, 1].map((i) => tx.getOutput(i).amount)).toEqual([30500n, 68500n]);
  });
  it.each([
    1n,
    3n,
    545n,
    1501n,
  ])("assigns leftover sats in recipient order for surplus %s", (surplus) => {
    const tx = decode(
      buildOnchainClaimTransaction({ ...params, htlcAmount: 98500n + surplus }),
    );
    expect(tx.outputsLength).toBe(2);
    expect(tx.getOutput(0).amount).toBe(30000n + surplus / 2n + (surplus % 2n));
    expect(tx.getOutput(1).amount).toBe(68000n + surplus / 2n);
    expect(tx.getOutput(0).amount! + tx.getOutput(1).amount! + 500n).toBe(
      98500n + surplus,
    );
  });
  it("distributes a surplus smaller than the recipient count without merging outputs", () => {
    const tx = decode(
      buildOnchainClaimTransaction({
        ...params,
        htlcAmount: 3502n,
        bitcoinPayout: {
          feeSats: "500",
          recipients: Array.from({ length: 3 }, () => ({
            address,
            amountSats: "1000",
          })),
        },
      }),
    );
    expect(tx.outputsLength).toBe(3);
    expect([0, 1, 2].map((i) => tx.getOutput(i).amount)).toEqual([
      1001n,
      1001n,
      1000n,
    ]);
  });
  it("allows a fee bump to exhaust surplus but never spend a commitment", () => {
    const tx = decode(
      buildOnchainClaimTransaction({
        ...params,
        bitcoinPayout: { ...payout, feeSats: "2000" },
      }),
    );
    expect([0, 1].map((i) => tx.getOutput(i).amount)).toEqual([30000n, 68000n]);
    expect(() =>
      buildOnchainClaimTransaction({
        ...params,
        bitcoinPayout: { ...payout, feeSats: "2001" },
      }),
    ).toThrow("cannot cover");
  });
  it("supports exact funding, duplicate addresses and fractional fee rates", () => {
    const tx = decode(
      buildOnchainClaimTransaction({
        ...params,
        htlcAmount: 98050n,
        feeRateSatPerVb: 0.1,
        bitcoinPayout: {
          feeSats: "50",
          recipients: payout.recipients.map((r) => ({ ...r, address })),
        },
      }),
    );
    expect(tx.outputsLength).toBe(2);
    expect(tx.getOutput(0).script).toEqual(tx.getOutput(1).script);
  });
  it("rejects shortfalls and insufficient fee rates without changing commitments", () => {
    for (const htlcAmount of [98000n, 98499n])
      expect(() =>
        buildOnchainClaimTransaction({ ...params, htlcAmount }),
      ).toThrow();
    for (const feeRateSatPerVb of [10, -1, NaN, Infinity])
      expect(() =>
        buildOnchainClaimTransaction({ ...params, feeRateSatPerVb }),
      ).toThrow();
    expect(payout.recipients[0].amountSats).toBe("30000");
  });
  it("rejects invalid commitments before returning a signed transaction", () => {
    const mainnet = btc.p2wpkh(secp256k1.getPublicKey(key)).address!;
    for (const recipients of [
      [],
      Array(101).fill(payout.recipients[0]),
      [{ address: mainnet, amountSats: "1000" }],
      [{ address, amountSats: "545" }],
      [{ address, amountSats: "-1" }],
      [{ address, amountSats: "1.5" }],
      [{ address, amountSats: "2100000000000001" }],
    ])
      expect(() =>
        buildOnchainClaimTransaction({
          ...params,
          bitcoinPayout: { ...payout, recipients },
        }),
      ).toThrow();
    for (const feeSats of ["0", "-1", "100001", "1.5"])
      expect(() =>
        buildOnchainClaimTransaction({
          ...params,
          bitcoinPayout: { ...payout, feeSats },
        }),
      ).toThrow();
  });
  it("preserves ordinary single-destination claiming", () => {
    const tx = decode(
      buildOnchainClaimTransaction({
        ...params,
        bitcoinPayout: undefined,
        feeRateSatPerVb: 2,
      }),
    );
    expect(tx.outputsLength).toBe(1);
    expect(tx.getOutput(0).amount).toBe(99700n);
    expect(tx.getInput(0).sequence).toBe(0xffffffff);
  });
  it("retains commitments through SQLite updates and a JSON backup restore", async () => {
    const storage = new SqliteSwapStorage(":memory:");
    const restored = new SqliteSwapStorage(":memory:");
    try {
      const record = storedSwap({ bitcoinPayout: payout });
      await storage.store(record);
      await storage.update(record.swapId, record.response);
      const backup = JSON.stringify(await storage.getAll());
      await restored.store(JSON.parse(backup)[0]);
      const recovered = await restored.get(record.swapId);
      expect(recovered?.bitcoinPayout).toEqual(payout);
      expect(
        decode(
          buildOnchainClaimTransaction({
            ...params,
            bitcoinPayout: recovered!.bitcoinPayout,
          }),
        ).outputsLength,
      ).toBe(2);
    } finally {
      storage.close();
      restored.close();
    }
  });
});

async function payoutClaimClient() {
  const storage = new InMemorySwapStorage();
  const client = await Client.builder()
    .withSwapStorage(storage)
    .withEsploraUrl("https://example.invalid")
    .build();
  const response = {
    direction: "evm_to_bitcoin",
    btc_htlc_address: buildOnchainClaimTransaction(params).htlcAddress,
    btc_hash_lock: params.hashLock,
    btc_refund_locktime: params.refundLocktime,
    btc_server_refund_pk: params.serverRefundPubKey,
    btc_fund_txid: params.fundingTxId,
    btc_fund_vout: 0,
    btc_htlc_script_version: 1,
    network: "signet",
    target_btc_address: second,
  };
  const record = storedSwap({
    response: response as never,
    bitcoinPayout: payout,
    publicKey: hex.encode(secp256k1.getPublicKey(key)),
    secretKey: hex.encode(key),
    preimage: hex.encode(preimage),
    targetAddress: address,
  });
  await storage.store(record);
  vi.spyOn(client, "getSwap").mockResolvedValue(response as never);
  vi.spyOn(esplora, "fetchTransactionOutputs").mockResolvedValue({
    vout: [{ value: 100000 }],
  });
  return { client, record, storage };
}

it("Client.claim preserves stored recipients after a response refresh", async () => {
  const { client, record } = await payoutClaimClient();
  vi.spyOn(esplora, "fetchFeeRateEstimate").mockResolvedValue(2);
  const broadcast = vi
    .spyOn(esplora, "broadcastTransactionWithRetry")
    .mockResolvedValue("txid");
  const result = await client.claim(record.swapId);
  expect(result.success).toBe(true);
  const tx = btc.Transaction.fromRaw(hex.decode(broadcast.mock.calls[0][1]));
  expect([0, 1].map((i) => tx.getOutput(i).amount)).toEqual([30750n, 68750n]);
  expect(tx.outputsLength).toBe(2);
});

it("Client.claim rejects an underpriced payout claim without broadcasting", async () => {
  const { client, record, storage } = await payoutClaimClient();
  // 500 committed sats cannot cover a two-output claim at 50 sat/vB.
  vi.spyOn(esplora, "fetchFeeRateEstimate").mockResolvedValue(50);
  const esploraBroadcast = vi
    .spyOn(esplora, "broadcastTransactionWithRetry")
    .mockResolvedValue("txid");
  const electrumBroadcast = vi
    .spyOn(electrum, "electrumBroadcastTransaction")
    .mockResolvedValue("txid");
  const result = await client.claim(record.swapId);
  expect(result.success).toBe(false);
  expect(result.message).toContain(
    "Committed fee does not meet the requested fee rate",
  );
  expect(result.txHash).toBeUndefined();
  expect(esploraBroadcast).not.toHaveBeenCalled();
  expect(electrumBroadcast).not.toHaveBeenCalled();
  expect((await storage.get(record.swapId))?.bitcoinPayout).toEqual(payout);
});
