import { afterEach, describe, expect, it, vi } from "vitest";
import { Client, InMemorySwapStorage } from "../src/index.js";
import {
  HASH_LOCK,
  HTLC,
  SERVER,
  SIGNER_ADDRESS,
  SWAP_ID,
  serverAnswering,
  signerWith,
  storedSwap,
  TIMELOCK,
  WBTC,
} from "./evm-funding-fixture.js";

const REFUND_BY_SIG_SELECTOR = "0x072c420d";
const NATIVE_REFUND_BY_SIG_SELECTOR = "0x3bab851e";
const TX = `0x${"ab".repeat(32)}`;

function params(
  overrides: Partial<Record<string, unknown>> = {},
): Record<string, unknown> {
  return {
    coordinator_address: "0x3a4cbee89a62c45d57c774527f931f0b67212c28",
    server_signer_address: SERVER,
    preimage_hash: HASH_LOCK,
    amount: "20305",
    token: WBTC,
    claim_address: SERVER,
    timelock: TIMELOCK,
    chain_id: 137,
    sweep_token: WBTC,
    min_amount_out: "0",
    calls_hash: `0x${"00".repeat(32)}`,
    coordinator_version: 5,
    htlc_address: HTLC,
    htlc_version: 7,
    htlc_kind: "erc20",
    refund_path: "direct",
    ...overrides,
  };
}

function signature(
  overrides: Partial<Record<string, unknown>> = {},
): Record<string, unknown> {
  return {
    id: SWAP_ID,
    htlc_address: HTLC,
    htlc_version: 7,
    htlc_kind: "erc20",
    chain_id: 137,
    preimage_hash: HASH_LOCK,
    amount: "20305",
    token: WBTC,
    sender: SIGNER_ADDRESS,
    claim_address: SERVER,
    timelock: TIMELOCK,
    v: 27,
    r: `0x${"11".repeat(32)}`,
    s: `0x${"22".repeat(32)}`,
    ...overrides,
  };
}

async function clientWith(stored = storedSwap()) {
  const storage = new InMemorySwapStorage();
  await storage.store(stored);
  return Client.builder().withSwapStorage(storage).build();
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("collabRefundEvmWithSigner on a direct-refund HTLC", () => {
  it("sends refundBySig from the wallet with the server's signature", async () => {
    const fetchMock = serverAnswering({
      collabParams: params(),
      collabSignature: signature(),
    });
    const sent: { to: string; data: string }[] = [];
    const signer = signerWith({
      sendTransaction: async (tx) => {
        sent.push(tx);
        return TX;
      },
    });
    const client = await clientWith();

    const { txHash } = await client.collabRefundEvmWithSigner(SWAP_ID, signer);

    expect(txHash).toBe(TX);
    expect(sent).toHaveLength(1);
    expect(sent[0].to).toBe(HTLC);
    expect(sent[0].data.startsWith(REFUND_BY_SIG_SELECTOR)).toBe(true);
    // The signature request names the wallet as the depositor.
    const signatureCall = fetchMock.mock.calls.find((c) =>
      String(c[0] instanceof Request ? c[0].url : c[0]).includes("signature"),
    );
    const [input, init] = signatureCall ?? [];
    const raw =
      input instanceof Request
        ? await input.clone().text()
        : String((init as RequestInit | undefined)?.body);
    const body = JSON.parse(raw);
    expect(body.depositor_address.toLowerCase()).toBe(SIGNER_ADDRESS);
  });

  it("encodes the native refundBySig for a native lock", async () => {
    serverAnswering({
      collabParams: params({ htlc_kind: "native", htlc_version: 2 }),
      collabSignature: signature({
        htlc_kind: "native",
        htlc_version: 2,
        token: "0x0000000000000000000000000000000000000000",
      }),
    });
    const sent: { to: string; data: string }[] = [];
    const signer = signerWith({
      sendTransaction: async (tx) => {
        sent.push(tx);
        return TX;
      },
    });
    const client = await clientWith();

    await client.collabRefundEvmWithSigner(SWAP_ID, signer);

    expect(sent[0].data.startsWith(NATIVE_REFUND_BY_SIG_SELECTOR)).toBe(true);
  });

  it("follows a speed-up but refuses a cancel", async () => {
    const REPLACED = `0x${"ef".repeat(32)}`;
    let sentCall: { to: string; data: string } | undefined;
    const replacedBy = (same: boolean) =>
      signerWith({
        sendTransaction: async (tx) => {
          sentCall = tx;
          return TX;
        },
        waitForReceipt: async () => ({
          status: "success",
          blockNumber: 1n,
          transactionHash: REPLACED,
          logs: [],
        }),
        getTransaction: async (hash) =>
          hash === REPLACED && same && sentCall
            ? { to: sentCall.to, input: sentCall.data, from: SIGNER_ADDRESS }
            : { to: SIGNER_ADDRESS, input: "0x", from: SIGNER_ADDRESS },
      });

    serverAnswering({ collabParams: params(), collabSignature: signature() });
    const client = await clientWith();
    const spedUp = await client.collabRefundEvmWithSigner(
      SWAP_ID,
      replacedBy(true),
    );
    expect(spedUp.txHash).toBe(REPLACED);

    serverAnswering({ collabParams: params(), collabSignature: signature() });
    await expect(
      client.collabRefundEvmWithSigner(SWAP_ID, replacedBy(false)),
    ).rejects.toThrow("may have cancelled");
  });

  it("refuses a signer on another chain before sending", async () => {
    serverAnswering({
      collabParams: params(),
      collabSignature: signature(),
    });
    const signer = signerWith({ chainId: 42161 });
    const client = await clientWith();

    await expect(
      client.collabRefundEvmWithSigner(SWAP_ID, signer),
    ).rejects.toThrow("connected to chain 42161");
  });

  it("explains an HTLC version with no early refund", async () => {
    serverAnswering({
      collabParams: params({ refund_path: "unavailable", htlc_version: 6 }),
    });
    const client = await clientWith();

    await expect(
      client.collabRefundEvmWithSigner(SWAP_ID, signerWith()),
    ).rejects.toThrow("erc20 HTLC v6");
  });
});

describe("collabRefundEvmSwap on a direct-refund HTLC", () => {
  it("needs the paymaster, so it refuses off Arbitrum", async () => {
    serverAnswering({ collabParams: params() });
    const client = await clientWith();

    await expect(client.collabRefundEvmSwap(SWAP_ID)).rejects.toThrow(
      "no paymaster sponsors it",
    );
  });
});
