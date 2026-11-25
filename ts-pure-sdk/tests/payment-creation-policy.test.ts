import { secp256k1 } from "@noble/curves/secp256k1.js";
import { ripemd160 } from "@noble/hashes/legacy.js";
import { hex } from "@scure/base";
import * as btc from "@scure/btc-signer";
import { afterEach, expect, it, vi } from "vitest";
import { Client } from "../src/client.js";
import { InMemorySwapStorage } from "../src/storage/index.js";
import { swapResponse } from "./evm-funding-fixture.js";

// No mock of the claim builder: creation simulates a real claim before it
// stores anything, so the server must echo a hash lock that matches.
afterEach(() => vi.unstubAllGlobals());
const address = btc.p2wpkh(
  secp256k1.getPublicKey(new Uint8Array(32).fill(2)),
  btc.TEST_NETWORK,
).address!;
async function setup(seconds = 120) {
  const storage = new InMemorySwapStorage();
  const bodies: Record<string, unknown>[] = [];
  const fetchMock = vi.fn(
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(input, init);
      if (new URL(req.url).pathname === "/swap/evm/bitcoin") {
        const body = await req.json();
        bodies.push(body);
        return Response.json({
          ...swapResponse(),
          network: "signet",
          btc_server_refund_pk: "02" + "11".repeat(32),
          btc_refund_locktime: Math.floor(Date.now() / 1000) + seconds,
          client_evm_address: body.user_address,
          target_btc_address: body.target_address,
          // HASH160(preimage) = RIPEMD160(SHA256(preimage)); hash_lock is the SHA256.
          btc_hash_lock: hex.encode(
            ripemd160(hex.decode(String(body.hash_lock).replace(/^0x/, ""))),
          ),
          btc_htlc_script_version: 1,
        });
      }
      return new Response("", { status: 404 });
    },
  );
  vi.stubGlobal("fetch", fetchMock);
  const client = await Client.builder()
    .withBaseUrl("https://api.test")
    .withEsploraUrl("https://explorer.test/api")
    .withSwapStorage(storage)
    .build();
  return { client, storage, bodies };
}
const options = {
  source: {
    chain: "42161",
    tokenId: "0x1111111111111111111111111111111111111111",
  },
  target: { chain: "Bitcoin", tokenId: "btc" },
  targetAmount: 10100,
  targetAddress: address,
  gasless: true,
  bitcoinPayout: {
    recipients: [{ address, amountSats: "10000" }],
    feeSats: "100",
    maxSlippageBps: 100,
  },
};
it("forwards an explicit fractional policy through generic creation", async () => {
  const { client, storage } = await setup();
  await client.createSwap({ ...options, minimumRelayFeeRateSatPerVb: 0.1 });
  expect(await storage.getAll()).toHaveLength(1);
});
it("does not leave a fundable local record after failed relay validation", async () => {
  const { client, storage } = await setup();
  await expect(client.createSwap(options)).rejects.toThrow("relay policy");
  expect(await storage.getAll()).toHaveLength(0);
});
it("rejects a dangerously short deadline before storing funding instructions", async () => {
  const { client, storage } = await setup(10);
  await expect(
    client.createSwap({ ...options, minimumRelayFeeRateSatPerVb: 0.1 }),
  ).rejects.toThrow("preimage");
  expect(await storage.getAll()).toHaveLength(0);
});
it("commits recipient floors and asks the server for floors plus the fee", async () => {
  const { client, storage, bodies } = await setup();
  await client.createSwap({ ...options, minimumRelayFeeRateSatPerVb: 0.1 });
  // 10,000 nominal at 1% tolerance commits 9,900; the fee stays 100.
  expect(bodies[0].minimum_payout_sats).toBe(10000);
  expect(bodies[0].amount_out).toBe(10100);
  const [stored] = await storage.getAll();
  expect(stored.bitcoinPayout).toEqual({
    recipients: [{ address, amountSats: "9900" }],
    feeSats: "100",
  });
});
it("rejects a target amount that is not the nominal total plus the fee", async () => {
  const { client, storage, bodies } = await setup();
  await expect(
    client.createSwap({
      ...options,
      targetAmount: 10000,
      minimumRelayFeeRateSatPerVb: 0.1,
    }),
  ).rejects.toThrow("Payment total must equal recipients plus the Bitcoin fee");
  expect(bodies).toHaveLength(0);
  expect(await storage.getAll()).toHaveLength(0);
});
it("refuses a committed fee that cannot meet the relay minimum without surplus", async () => {
  const { client, storage } = await setup();
  // A one-output claim is over 100 vB, so 100 sats cannot pay 2 sat/vB.
  await expect(
    client.createSwap({ ...options, minimumRelayFeeRateSatPerVb: 2 }),
  ).rejects.toThrow("below the relay minimum of 2 sat/vB");
  expect(await storage.getAll()).toHaveLength(0);
});
