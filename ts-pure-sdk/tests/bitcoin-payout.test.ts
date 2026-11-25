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
  // Far enough out that the payment claim window never refuses.
  refundLocktime: 4_000_000_000,
  destinationAddress: address,
  feeRateSatPerVb: 1,
  network: "signet" as const,
  bitcoinPayout: payout,
};
const decode = (result: ReturnType<typeof buildOnchainClaimTransaction>) =>
  btc.Transaction.fromRaw(hex.decode(result.txHex));

describe("multi-recipient Bitcoin claim primitive", () => {
  it("preserves recipient order and splits surplus in proportion", () => {
    const result = buildOnchainClaimTransaction(params);
    const tx = decode(result);
    expect(tx.outputsLength).toBe(2);
    // 1,500 surplus: 459.18 and 1,040.81, the leftover sat to the first.
    expect([0, 1].map((i) => tx.getOutput(i).amount)).toEqual([30460n, 69040n]);
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
    // 1,000 surplus: 306.12 and 693.87, the leftover sat to the first.
    expect([0, 1].map((i) => tx.getOutput(i).amount)).toEqual([30307n, 68693n]);
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
    const shares = [30000n, 68000n].map((a) => (surplus * a) / 98000n);
    const leftover = surplus - shares[0] - shares[1];
    expect(leftover).toBeLessThan(2n);
    expect(tx.getOutput(0).amount).toBe(30000n + shares[0] + leftover);
    expect(tx.getOutput(1).amount).toBe(68000n + shares[1]);
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
  it("rejects shortfalls and unusable fee rates without changing commitments", () => {
    for (const htlcAmount of [98000n, 98499n])
      expect(() =>
        buildOnchainClaimTransaction({ ...params, htlcAmount }),
      ).toThrow("cannot cover the committed recipients and fee");
    for (const feeRateSatPerVb of [0, -1, NaN, Infinity])
      expect(() =>
        buildOnchainClaimTransaction({ ...params, feeRateSatPerVb }),
      ).toThrow("Fee rate must be a positive number");
    // 1,500 sats of surplus cannot pay 100 sat/vB.
    expect(() =>
      buildOnchainClaimTransaction({ ...params, feeRateSatPerVb: 100 }),
    ).toThrow("Surplus cannot cover");
    expect(payout.recipients[0].amountSats).toBe("30000");
    expect(payout.feeSats).toBe("500");
  });
  it("raises the committed fee out of surplus to meet the fee rate", () => {
    const committed = buildOnchainClaimTransaction(params);
    const result = buildOnchainClaimTransaction({
      ...params,
      feeRateSatPerVb: 10,
    });
    expect(result.vsize).toBe(committed.vsize);
    expect(result.fee).toBe(BigInt(committed.vsize * 10));
    const tx = decode(result);
    const amounts = [0, 1].map((i) => tx.getOutput(i).amount ?? 0n);
    expect(amounts[0]).toBeGreaterThanOrEqual(30000n);
    expect(amounts[1]).toBeGreaterThanOrEqual(68000n);
    expect(amounts[0] + amounts[1] + result.fee).toBe(100000n);
    expect(result.txId).not.toBe(committed.txId);
    expect(payout.feeSats).toBe("500");
  });
  it("refuses a fee rate that would exceed the payout fee cap", () => {
    expect(() =>
      buildOnchainClaimTransaction({
        ...params,
        htlcAmount: 10_000_000n,
        feeRateSatPerVb: 1000,
      }),
    ).toThrow("above the 100000-sat payout fee cap");
  });
  it("meets a fractional rate with the exact fee", () => {
    // 915 * 2.2 is 2013.0000000000002 in floating point.
    const at = (feeSats: string) =>
      buildOnchainClaimTransaction({
        ...params,
        feeRateSatPerVb: 2.2,
        bitcoinPayout: {
          feeSats,
          recipients: Array.from({ length: 26 }, () => ({
            address,
            amountSats: "1000",
          })),
        },
      });
    expect(at("2013").vsize).toBe(915);
    expect(at("2013").fee).toBe(2013n);
    expect(at("2012").fee).toBe(2013n);
  });
  it("accepts 100 recipients and rejects 101 for the recipient limit", () => {
    const recipients = (length: number) =>
      Array.from({ length }, () => ({ address, amountSats: "546" }));
    const tx = decode(
      buildOnchainClaimTransaction({
        ...params,
        bitcoinPayout: { feeSats: "5000", recipients: recipients(100) },
      }),
    );
    expect(tx.outputsLength).toBe(100);
    expect(() =>
      buildOnchainClaimTransaction({
        ...params,
        bitcoinPayout: { feeSats: "5000", recipients: recipients(101) },
      }),
    ).toThrow("A Bitcoin payout needs 1–100 recipients");
  });
  it("rejects invalid commitments before returning a signed transaction", () => {
    const mainnet = btc.p2wpkh(secp256k1.getPublicKey(key)).address!;
    const cases: [unknown[], string][] = [
      [[], "A Bitcoin payout needs 1–100 recipients"],
      [
        [{ address: mainnet, amountSats: "1000" }],
        "not a valid address for this network",
      ],
      [[{ address, amountSats: "545" }], "at least 546 sats"],
      [[{ address, amountSats: "-1" }], "integer satoshi strings"],
      [[{ address, amountSats: "1.5" }], "integer satoshi strings"],
      [[{ address, amountSats: "2100000000000001" }], "exceeds Bitcoin supply"],
    ];
    for (const [recipients, message] of cases)
      expect(() =>
        buildOnchainClaimTransaction({
          ...params,
          bitcoinPayout: { ...payout, recipients } as typeof payout,
        }),
      ).toThrow(message);
    for (const feeSats of ["0", "100001"])
      expect(() =>
        buildOnchainClaimTransaction({
          ...params,
          bitcoinPayout: { ...payout, feeSats },
        }),
      ).toThrow("Payout fee must be between 1 and 100,000 sats");
    for (const feeSats of ["-1", "1.5"])
      expect(() =>
        buildOnchainClaimTransaction({
          ...params,
          bitcoinPayout: { ...payout, feeSats },
        }),
      ).toThrow("integer satoshi strings");
  });
  it("rejects a null payout instead of claiming to the single destination", () => {
    expect(() =>
      buildOnchainClaimTransaction({
        ...params,
        bitcoinPayout: null as never,
      }),
    ).toThrow("A Bitcoin payout must be an object");
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

async function payoutClaimClient(targetAddress: string | undefined = address) {
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
    targetAddress,
  });
  await storage.store(record);
  vi.spyOn(client, "getSwap").mockResolvedValue(response as never);
  vi.spyOn(esplora, "fetchTransactionOutputs").mockResolvedValue({
    vout: [{ value: 100000 }],
  });
  vi.spyOn(esplora, "fetchMinimumRelayFeeRate").mockResolvedValue(1);
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
  expect([0, 1].map((i) => tx.getOutput(i).amount)).toEqual([30460n, 69040n]);
  expect(tx.outputsLength).toBe(2);
});

it("Client.claim rejects a payout claim the surplus cannot price without broadcasting", async () => {
  const { client, record, storage } = await payoutClaimClient();
  // 1,500 sats of surplus cannot pay a two-output claim at 50 sat/vB.
  vi.spyOn(esplora, "fetchFeeRateEstimate").mockResolvedValue(50);
  const esploraBroadcast = vi
    .spyOn(esplora, "broadcastTransactionWithRetry")
    .mockResolvedValue("txid");
  const electrumBroadcast = vi
    .spyOn(electrum, "electrumBroadcastTransaction")
    .mockResolvedValue("txid");
  const result = await client.claim(record.swapId);
  expect(result.success).toBe(false);
  expect(result.message).toContain("Surplus cannot cover");
  expect(result.txHash).toBeUndefined();
  expect(esploraBroadcast).not.toHaveBeenCalled();
  expect(electrumBroadcast).not.toHaveBeenCalled();
  expect((await storage.get(record.swapId))?.bitcoinPayout).toEqual(payout);
});

it("Client.claim bumps a payout claim when called again at a higher rate", async () => {
  const { client, record, storage } = await payoutClaimClient();
  vi.spyOn(esplora, "fetchFeeRateEstimate").mockResolvedValue(3);
  const broadcast = vi
    .spyOn(esplora, "broadcastTransactionWithRetry")
    .mockResolvedValue("txid");
  expect((await client.claim(record.swapId)).success).toBe(true);
  expect(
    (await client.claim(record.swapId, { feeRateSatPerVb: 9 })).success,
  ).toBe(true);
  const [first, second] = broadcast.mock.calls.map((call) =>
    btc.Transaction.fromRaw(hex.decode(call[1])),
  );
  const outputs = (tx: btc.Transaction) =>
    [0, 1].map((i) => tx.getOutput(i).amount ?? 0n);
  const fee = (tx: btc.Transaction) =>
    100000n - outputs(tx).reduce((a, b) => a + b, 0n);
  expect(fee(first)).toBe(BigInt(first.vsize * 3));
  expect(fee(second)).toBe(BigInt(second.vsize * 9));
  expect(second.getInput(0).txid).toEqual(first.getInput(0).txid);
  expect(outputs(second)[0]).toBeGreaterThanOrEqual(30000n);
  expect(outputs(second)[1]).toBeGreaterThanOrEqual(68000n);
  expect((await storage.get(record.swapId))?.bitcoinPayout).toEqual(payout);
});

it("Client.claim pays stored recipients without a target address", async () => {
  const { client, record } = await payoutClaimClient(undefined);
  vi.spyOn(esplora, "fetchFeeRateEstimate").mockResolvedValue(2);
  const broadcast = vi
    .spyOn(esplora, "broadcastTransactionWithRetry")
    .mockResolvedValue("txid");
  const result = await client.claim(record.swapId);
  expect(result.success).toBe(true);
  const tx = btc.Transaction.fromRaw(hex.decode(broadcast.mock.calls[0][1]));
  expect(tx.outputsLength).toBe(2);
});

it.each([
  [
    "an explicit destination",
    { destinationAddress: address },
    "cannot override",
  ],
  [
    "a zero fee rate",
    { feeRateSatPerVb: 0 },
    "Fee rate must be a positive number",
  ],
])("Client.claim refuses %s on a payout swap without broadcasting", async (_, options, message) => {
  const { client, record, storage } = await payoutClaimClient();
  vi.spyOn(esplora, "fetchFeeRateEstimate").mockResolvedValue(2);
  const broadcast = vi
    .spyOn(esplora, "broadcastTransactionWithRetry")
    .mockResolvedValue("txid");
  const result = await client.claim(record.swapId, options);
  expect(result.success).toBe(false);
  expect(result.message).toContain(message);
  expect(broadcast).not.toHaveBeenCalled();
  expect((await storage.get(record.swapId))?.bitcoinPayout).toEqual(payout);
});
