import { hexToBytes } from "@noble/hashes/utils.js";
import { bech32 } from "@scure/base";
import { encodeFunctionData, parseAbi, recoverMessageAddress } from "viem";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoicePaymentHash } from "../src/bolt11.js";
import {
  decodeSwapCreatedLog,
  htlcSwapKey,
  SWAP_REHASHED_TOPIC,
} from "../src/evm/htlc.js";
import {
  deriveEvmAddress,
  eip191Digest,
  signEvmMessage,
} from "../src/evm/signing.js";
import {
  Client,
  InMemorySwapStorage,
  InMemoryWalletStorage,
  RehashNotRecordedError,
  rehashAuthorizationMessage,
} from "../src/index.js";
import {
  signerWith as baseSignerWith,
  COORDINATOR,
  FUND_TXID,
  HASH_LOCK,
  HTLC,
  HTLC_REFUND_SELECTOR,
  logFrom,
  REAL_LOG,
  SERVER,
  SIGNER_ADDRESS,
  SWAP_ID,
  serverAnswering,
  serverGone,
  storedSwap,
  swapResponse,
  TIMELOCK,
  USDC,
  WBTC,
} from "./evm-funding-fixture.js";

// BOLT 11 spec example: "Please send $3 for a cup of coffee to the same
// peer, within one minute"; payment hash 0001020304...0102.
const INVOICE =
  "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53ut353s06fv3qfegext0eh0ymjpf39tuven09sam30g4vgpfna3rh";
const PAYMENT_HASH =
  "0x0001020304050607080900010203040506070809000102030405060708090102";
const NEW_TIMELOCK = TIMELOCK + 7200;
const REHASH_SELECTOR = "0x5a83bf7f";
const TX = `0x${"cd".repeat(32)}`;

describe("invoicePaymentHash", () => {
  it("reads the p field of a BOLT 11 invoice", () => {
    expect(invoicePaymentHash(INVOICE)).toBe(PAYMENT_HASH);
    expect(invoicePaymentHash(`lightning:${INVOICE.toUpperCase()}`)).toBe(
      PAYMENT_HASH,
    );
  });

  it("rejects what is not an invoice", () => {
    expect(() =>
      invoicePaymentHash("bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh"),
    ).toThrow("Not a Lightning invoice");
    expect(() => invoicePaymentHash("lnbc1qqqqqqqqqqqqqqqqqqq")).toThrow();
  });
});

describe("signEvmMessage", () => {
  it("is a personal_sign signature viem recovers to the key's address", async () => {
    const key = new Uint8Array(32).fill(0x0d);
    const message = rehashAuthorizationMessage(SWAP_ID, PAYMENT_HASH);
    const signature = signEvmMessage(key, message);
    expect(signature).toMatch(/^0x[0-9a-f]{130}$/);
    const recovered = await recoverMessageAddress({
      message,
      signature: signature as `0x${string}`,
    });
    expect(recovered.toLowerCase()).toBe(deriveEvmAddress(key).toLowerCase());
    expect(eip191Digest(message)).toMatch(/^0x[0-9a-f]{64}$/);
  });

  it("spells the message the way the server rebuilds it", () => {
    expect(rehashAuthorizationMessage(SWAP_ID, PAYMENT_HASH.slice(2))).toBe(
      `Satora rehash\nswap: ${SWAP_ID}\ninvoice payment hash: ${PAYMENT_HASH}`,
    );
  });
});

function rehashParams(overrides: Record<string, unknown> = {}) {
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
    new_timelock: NEW_TIMELOCK,
    max_invoice_amount_sats: 20001,
    min_invoice_validity_secs: 1800,
    ...overrides,
  };
}

function rehashSignature(overrides: Record<string, unknown> = {}) {
  return {
    ...rehashParams(),
    new_preimage_hash: PAYMENT_HASH,
    v: 28,
    r: `0x${"11".repeat(32)}`,
    s: `0x${"22".repeat(32)}`,
    ...overrides,
  };
}

async function clientWith(
  stored = storedSwap({ direction: "evm_to_lightning" }),
) {
  const storage = new InMemorySwapStorage();
  await storage.store(stored);
  return Client.builder().withSwapStorage(storage).build();
}

/** The depositor's wallet; on v7 it is the lock's sender. */
function signerWith(
  overrides: Parameters<typeof baseSignerWith>[0] = {},
): ReturnType<typeof baseSignerWith> {
  return baseSignerWith({ logs: [logFrom(SIGNER_ADDRESS)], ...overrides });
}

/** {@link REAL_LOG} as a native lock: a zero asset word. */
function nativeLogFrom(sender: string): typeof REAL_LOG {
  const data = REAL_LOG.data.replace(/^0x/, "");
  return {
    ...logFrom(sender),
    data: `0x${"0".repeat(64)}${data.slice(64)}`,
  };
}

/**
 * {@link REAL_LOG} as the HTLC emits it for a lock `sender` created under
 * `hash` / `timelock`: its key is the contract's (see htlc-swap-key.test.ts).
 */
function ownLog(
  sender: string,
  hash = HASH_LOCK,
  timelock = TIMELOCK,
): typeof REAL_LOG {
  const log = logFrom(sender);
  const key = htlcSwapKey({
    preimageHash: hash,
    amount: 20305n,
    token: WBTC,
    refundAddress: sender,
    claimAddress: SERVER,
    timelock,
  });
  const data = REAL_LOG.data.replace(/^0x/, "");
  return {
    ...log,
    topics: [log.topics[0], hash, ...log.topics.slice(2)],
    data: `0x${data.slice(0, 64)}${data.slice(64, 128)}${timelock
      .toString(16)
      .padStart(64, "0")}${key.slice(2)}`,
  };
}

/** {@link INVOICE} with another payment hash (its signature is not checked). */
function invoiceWithHash(hash: string): string {
  const { prefix, words } = bech32.decode(
    INVOICE as `${string}1${string}`,
    false,
  );
  const hashWords = bech32.toWords(hexToBytes(hash.slice(2)));
  // Walk the tagged fields `type(1) length(2) data(length)` after the
  // 7-word timestamp to the payment hash (tag 1).
  let i = 7;
  while (words[i] !== 1) i += 3 + ((words[i + 1] << 5) | words[i + 2]);
  const next = [
    ...words.slice(0, i + 3),
    ...hashWords,
    ...words.slice(i + 3 + hashWords.length),
  ];
  return bech32.encode(prefix, next, false);
}

afterEach(() => vi.unstubAllGlobals());

describe("retrySwapWithInvoiceWithSigner", () => {
  it("has the wallet authorize the invoice, then sends rehash from it", async () => {
    const fetchMock = serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    const signedRaw: string[] = [];
    const sent: { to: string; data: string }[] = [];
    const signer = signerWith({
      signMessage: async ({ raw }) => {
        signedRaw.push(raw);
        return `0x${"ab".repeat(65)}`;
      },
      sendTransaction: async (tx) => {
        sent.push(tx);
        return TX;
      },
    });
    const client = await clientWith();

    const result = await client.retrySwapWithInvoiceWithSigner(
      SWAP_ID,
      INVOICE,
      signer,
    );

    expect(result).toEqual({
      id: SWAP_ID,
      txHash: TX,
      newPreimageHash: PAYMENT_HASH,
      newTimelock: NEW_TIMELOCK,
    });
    // The wallet signed exactly the server's message, as UTF-8 bytes.
    expect(signedRaw).toHaveLength(1);
    expect(Buffer.from(signedRaw[0].slice(2), "hex").toString("utf8")).toBe(
      rehashAuthorizationMessage(SWAP_ID, PAYMENT_HASH),
    );
    // The request carried the invoice, the wallet and its signature
    // (the params GET comes first; the POST is the last call).
    const [input, init] = fetchMock.mock.calls.at(-1) as [
      RequestInfo | URL,
      RequestInit | undefined,
    ];
    const raw =
      input instanceof Request
        ? await input.clone().text()
        : String(init?.body);
    const body = JSON.parse(raw);
    expect(body.invoice).toBe(INVOICE);
    expect(body.depositor_address.toLowerCase()).toBe(SIGNER_ADDRESS);
    expect(body.depositor_signature).toBe(`0x${"ab".repeat(65)}`);
    // And the lock moved on chain.
    expect(sent).toHaveLength(1);
    expect(sent[0].to).toBe(HTLC);
    expect(sent[0].data.startsWith(REHASH_SELECTOR)).toBe(true);
    expect(sent[0].data).toContain(PAYMENT_HASH.slice(2));
  });

  it("encodes the native rehash for a native lock", async () => {
    serverAnswering({
      rehashParams: rehashParams({ htlc_kind: "native", htlc_version: 2 }),
      rehashSignature: rehashSignature({
        htlc_kind: "native",
        htlc_version: 2,
        token: "0x0000000000000000000000000000000000000000",
      }),
    });
    const sent: { to: string; data: string }[] = [];
    const signer = signerWith({
      logs: [nativeLogFrom(SIGNER_ADDRESS)],
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async (tx) => {
        sent.push(tx);
        return TX;
      },
    });
    const client = await clientWith();

    await client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer);

    expect(sent[0].data.startsWith("0x6eecb3d8")).toBe(true);
  });

  it("refuses a server signature for another payment hash", async () => {
    serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature({
        new_preimage_hash: `0x${"99".repeat(32)}`,
      }),
    });
    const sent: unknown[] = [];
    const signer = signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async (tx) => {
        sent.push(tx);
        return TX;
      },
    });
    const client = await clientWith();

    await expect(
      client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer),
    ).rejects.toThrow("not to the invoice's payment hash");
    expect(sent).toHaveLength(0);
  });

  it("remembers the new terms so a later refund targets the moved lock", async () => {
    serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    const signer = signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async () => TX,
    });
    const storage = new InMemorySwapStorage();
    await storage.store(storedSwap({ direction: "evm_to_lightning" }));
    const client = await Client.builder().withSwapStorage(storage).build();

    await client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer);

    expect((await storage.get(SWAP_ID))?.evmRehashes).toEqual([
      {
        txid: TX,
        fromHashLock: HASH_LOCK,
        hashLock: PAYMENT_HASH,
        timelock: NEW_TIMELOCK,
      },
    ]);
    // A server refresh rewrites only the response, never the record.
    const stored = await storage.get(SWAP_ID);
    if (!stored) throw new Error("swap not stored");
    await storage.update(SWAP_ID, stored.response);
    expect((await storage.get(SWAP_ID))?.evmRehashes?.[0]?.hashLock).toBe(
      PAYMENT_HASH,
    );
  });

  it("lets the wallet refund the moved lock afterwards, without the server", async () => {
    serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    // The funding tx created the lock from this wallet; the rehash tx moved it.
    const created = logFrom(SIGNER_ADDRESS);
    const key = decodeSwapCreatedLog(created)?.key ?? "";
    const rehashed = {
      address: HTLC,
      topics: [SWAP_REHASHED_TOPIC, HASH_LOCK, key, `0x${"77".repeat(32)}`],
      data: PAYMENT_HASH,
    };
    const receipts: Record<string, (typeof created)[]> = {
      [FUND_TXID]: [created],
      [TX]: [rehashed],
    };
    const signer = signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async () => TX,
      waitForReceipt: async (hash) => ({
        status: "success",
        blockNumber: 1n,
        transactionHash: hash,
        logs: receipts[hash] ?? [],
      }),
    });
    const storage = new InMemorySwapStorage();
    await storage.store(storedSwap({ direction: "evm_to_lightning" }));
    const client = await Client.builder().withSwapStorage(storage).build();
    await client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer);

    // The server is not consulted: the refund comes from the two receipts.
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new Error("server must not be used");
      }),
    );
    const result = await client.refundSwap(SWAP_ID, { signer });

    expect(result.success).toBe(true);
    const refund = result.evmRefundData;
    if (!refund) throw new Error(result.message);
    expect(refund.to).toBe(HTLC);
    expect(refund.data.startsWith(HTLC_REFUND_SELECTOR)).toBe(true);
    expect(refund.data).toContain(PAYMENT_HASH.slice(2));
    expect(refund.data).not.toContain(HASH_LOCK.slice(2));
    expect(refund.timelockExpiry).toBe(NEW_TIMELOCK);
  });

  it("still refunds from the receipts after the server switched to the new hash", async () => {
    serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    const created = logFrom(SIGNER_ADDRESS);
    const key = decodeSwapCreatedLog(created)?.key ?? "";
    const rehashed = {
      address: HTLC,
      topics: [SWAP_REHASHED_TOPIC, HASH_LOCK, key, `0x${"77".repeat(32)}`],
      data: PAYMENT_HASH,
    };
    const receipts: Record<string, (typeof created)[]> = {
      [FUND_TXID]: [created],
      [TX]: [rehashed],
    };
    const signer = signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async () => TX,
      waitForReceipt: async (hash) => ({
        status: "success",
        blockNumber: 1n,
        transactionHash: hash,
        logs: receipts[hash] ?? [],
      }),
    });
    const storage = new InMemorySwapStorage();
    await storage.store(storedSwap({ direction: "evm_to_lightning" }));
    const client = await Client.builder().withSwapStorage(storage).build();
    await client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer);

    // The monitor confirmed the move: the server now reports the new hash,
    // the new timelock and the rehash tx as the funding tx.
    const stored = await storage.get(SWAP_ID);
    if (!stored) throw new Error("swap not stored");
    await storage.update(SWAP_ID, {
      ...stored.response,
      hash_lock: PAYMENT_HASH,
      evm_refund_locktime: NEW_TIMELOCK,
      evm_fund_txid: TX,
    } as typeof stored.response);
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new Error("server must not be used");
      }),
    );

    const result = await client.refundSwap(SWAP_ID, { signer });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.data).toContain(PAYMENT_HASH.slice(2));
    expect(result.evmRefundData?.timelockExpiry).toBe(NEW_TIMELOCK);
  });

  it("refuses to rehash a swap whose payment hash this client never recorded", async () => {
    // Recovered into fresh storage: the payment hash is only the server's
    // word, so nothing the client holds tells this swap's lock from another.
    const fetchMock = serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    const signed: unknown[] = [];
    const sent: unknown[] = [];
    const signer = signerWith({
      signMessage: async (message) => {
        signed.push(message);
        return `0x${"ab".repeat(65)}`;
      },
      sendTransaction: async (tx) => {
        sent.push(tx);
        return TX;
      },
    });
    const client = await clientWith(
      storedSwap({
        direction: "evm_to_lightning",
        evmFundTxid: undefined,
        lightningPaymentHash: undefined,
        responseFundTxid: FUND_TXID,
      }),
    );

    await expect(
      client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer),
    ).rejects.toThrow("no payment hash recorded");
    expect(signed).toHaveLength(0);
    expect(sent).toHaveLength(0);
    // The server was not asked to sign either.
    const urls = fetchMock.mock.calls.map(([input]) =>
      input instanceof Request ? input.url : String(input),
    );
    expect(urls.some((url) => /rehash-evm$/.test(url))).toBe(false);
  });

  it("keeps the rehash history when recovery from the seed rebuilds the swap", async () => {
    const storage = new InMemorySwapStorage();
    let fetchFor: "retry" | "recover" = "retry";
    const retryServer = serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    const client = await Client.builder()
      .withSignerStorage(new InMemoryWalletStorage())
      .withSwapStorage(storage)
      .build();
    const stored = storedSwap({ direction: "evm_to_lightning" });
    await storage.store(stored);
    const created = logFrom(SIGNER_ADDRESS);
    const key = decodeSwapCreatedLog(created)?.key ?? "";
    const receipts: Record<string, (typeof created)[]> = {
      [FUND_TXID]: [created],
      [TX]: [
        {
          address: HTLC,
          topics: [SWAP_REHASHED_TOPIC, HASH_LOCK, key, `0x${"77".repeat(32)}`],
          data: PAYMENT_HASH,
        },
      ],
    };
    const signer = signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async () => TX,
      waitForReceipt: async (hash) => ({
        status: "success",
        blockNumber: 1n,
        transactionHash: hash,
        logs: receipts[hash] ?? [],
      }),
    });
    // The server's recover answer carries the promoted terms.
    const promoted = {
      ...stored.response,
      hash_lock: PAYMENT_HASH,
      evm_refund_locktime: NEW_TIMELOCK,
      evm_fund_txid: TX,
    };
    retryServer.mockImplementation(async (input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : String(input);
      if (fetchFor === "retry") {
        if (url.includes("rehash-evm/params"))
          return Response.json(rehashParams());
        if (url.includes("rehash-evm")) return Response.json(rehashSignature());
      }
      if (url.endsWith("/swap/recover")) {
        return Response.json({
          swaps: [{ ...promoted, index: 0 }],
          highest_index: 0,
          next_index: 1,
          scanned_until: 0,
          complete: true,
        });
      }
      throw new Error(`unexpected request ${url}`);
    });

    await client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer);
    fetchFor = "recover";
    await client.recoverSwaps();

    expect((await storage.get(SWAP_ID))?.evmRehashes?.[0]?.hashLock).toBe(
      PAYMENT_HASH,
    );
    retryServer.mockImplementation(async () => {
      throw new Error("server must not be used");
    });
    const result = await client.refundSwap(SWAP_ID, { signer });
    expect(result.success).toBe(true);
    expect(result.evmRefundData?.data).toContain(PAYMENT_HASH.slice(2));
  });

  it("follows a second rehash to the lock that exists", async () => {
    const SECOND_HASH = `0x${"88".repeat(32)}`;
    const SECOND_TX = `0x${"89".repeat(32)}`;
    const SECOND_TIMELOCK = NEW_TIMELOCK + 7200;
    const created = logFrom(SIGNER_ADDRESS);
    const key0 = decodeSwapCreatedLog(created)?.key ?? "";
    const key1 = `0x${"77".repeat(32)}`;
    const key2 = `0x${"78".repeat(32)}`;
    const receipts: Record<string, (typeof created)[]> = {
      [FUND_TXID]: [created],
      [TX]: [
        {
          address: HTLC,
          topics: [SWAP_REHASHED_TOPIC, HASH_LOCK, key0, key1],
          data: PAYMENT_HASH,
        },
      ],
      [SECOND_TX]: [
        {
          address: HTLC,
          topics: [SWAP_REHASHED_TOPIC, PAYMENT_HASH, key1, key2],
          data: SECOND_HASH,
        },
      ],
    };
    const signer = signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async () => TX,
      waitForReceipt: async (hash) => ({
        status: "success",
        blockNumber: 1n,
        transactionHash: hash,
        logs: receipts[hash] ?? [],
      }),
    });
    serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    const storage = new InMemorySwapStorage();
    await storage.store(storedSwap({ direction: "evm_to_lightning" }));
    const client = await Client.builder().withSwapStorage(storage).build();
    await client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer);
    // The replacement failed too and a second invoice moved the lock again
    // (recorded here as the retry would record it).
    const afterFirst = await storage.get(SWAP_ID);
    if (!afterFirst?.evmRehashes) throw new Error("first rehash not recorded");
    await storage.store({
      ...afterFirst,
      evmRehashes: [
        ...afterFirst.evmRehashes,
        {
          txid: SECOND_TX,
          fromHashLock: PAYMENT_HASH,
          hashLock: SECOND_HASH,
          timelock: SECOND_TIMELOCK,
        },
      ],
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new Error("server must not be used");
      }),
    );

    const result = await client.refundSwap(SWAP_ID, { signer });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.data).toContain(SECOND_HASH.slice(2));
    expect(result.evmRefundData?.data).not.toContain(PAYMENT_HASH.slice(2));
    expect(result.evmRefundData?.timelockExpiry).toBe(SECOND_TIMELOCK);
  });

  it("refuses a signed timelock far beyond the advertised one", async () => {
    serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature({
        new_timelock: NEW_TIMELOCK + 7 * 24 * 3600,
      }),
    });
    const sent: unknown[] = [];
    const signer = signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async (tx) => {
        sent.push(tx);
        return TX;
      },
    });
    const client = await clientWith();

    await expect(
      client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer),
    ).rejects.toThrow("later than the advertised");
    expect(sent).toHaveLength(0);
  });

  it("records the rehash on broadcast, so a lost receipt wait does not lose the terms", async () => {
    serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    const created = logFrom(SIGNER_ADDRESS);
    const key = decodeSwapCreatedLog(created)?.key ?? "";
    const receipts: Record<string, (typeof created)[]> = {
      [FUND_TXID]: [created],
      [TX]: [
        {
          address: HTLC,
          topics: [SWAP_REHASHED_TOPIC, HASH_LOCK, key, `0x${"77".repeat(32)}`],
          data: PAYMENT_HASH,
        },
      ],
    };
    let rpcDown = true;
    const signer = signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async () => TX,
      waitForReceipt: async (hash) => {
        if (rpcDown && hash === TX) throw new Error("rpc timeout");
        return {
          status: "success",
          blockNumber: 1n,
          transactionHash: hash,
          logs: receipts[hash] ?? [],
        };
      },
    });
    const storage = new InMemorySwapStorage();
    await storage.store(storedSwap({ direction: "evm_to_lightning" }));
    const client = await Client.builder().withSwapStorage(storage).build();

    await expect(
      client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer),
    ).rejects.toThrow("rpc timeout");
    expect((await storage.get(SWAP_ID))?.evmRehashes?.[0]?.txid).toBe(TX);

    // The tx mined meanwhile; the refund follows the record to the new lock.
    rpcDown = false;
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new Error("server must not be used");
      }),
    );
    const result = await client.refundSwap(SWAP_ID, { signer });
    expect(result.success).toBe(true);
    expect(result.evmRefundData?.data).toContain(PAYMENT_HASH.slice(2));
  });

  it("surfaces a rehash it could not record instead of hiding it", async () => {
    serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    const signer = signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async () => TX,
    });
    const storage = new InMemorySwapStorage();
    await storage.store(storedSwap({ direction: "evm_to_lightning" }));
    const client = await Client.builder().withSwapStorage(storage).build();
    storage.store = async () => {
      throw new Error("disk full");
    };

    const failure = await client
      .retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer)
      .catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(RehashNotRecordedError);
    const notRecorded = failure as RehashNotRecordedError;
    expect(notRecorded.record).toEqual({
      txid: TX,
      fromHashLock: HASH_LOCK,
      hashLock: PAYMENT_HASH,
      timelock: NEW_TIMELOCK,
    });
    expect(notRecorded.message).toContain("disk full");
  });

  it("skips a recorded rehash that did not move the lock", async () => {
    serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    const created = logFrom(SIGNER_ADDRESS);
    const signer = signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async () => TX,
      // The rehash tx mined without moving anything.
      waitForReceipt: async (hash) => ({
        status: "success",
        blockNumber: 1n,
        transactionHash: hash,
        logs: hash === FUND_TXID ? [created] : [],
      }),
    });
    const storage = new InMemorySwapStorage();
    await storage.store(storedSwap({ direction: "evm_to_lightning" }));
    const client = await Client.builder().withSwapStorage(storage).build();
    await client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer);

    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new Error("server is gone");
      }),
    );
    const result = await client.refundSwap(SWAP_ID, { signer });

    // The lock is still under its original terms, and is refunded there.
    expect(result.success).toBe(true);
    expect(result.evmRefundData?.data).toContain(HASH_LOCK.slice(2));
    expect(result.evmRefundData?.data).not.toContain(PAYMENT_HASH.slice(2));
  });

  it("needs a signer that can personal_sign", async () => {
    serverAnswering({ rehashParams: rehashParams() });
    const client = await clientWith();

    await expect(
      client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signerWith()),
    ).rejects.toThrow("signMessage");
  });

  it("surfaces the server's refusal", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = input instanceof Request ? input.url : String(input);
        if (url.includes("/params")) return Response.json(rehashParams());
        return Response.json(
          { error: "Invoice asks for 30000 sats, more than the payout" },
          { status: 400 },
        );
      }),
    );
    const signer = signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
    });
    const client = await clientWith();

    await expect(
      client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer),
    ).rejects.toThrow("Rehash refused: Invoice asks for 30000 sats");
  });
});

describe("retrySwapWithInvoice", () => {
  it("needs the paymaster, so it refuses off Arbitrum", async () => {
    serverAnswering({ rehashParams: rehashParams() });
    const client = await clientWith();

    await expect(client.retrySwapWithInvoice(SWAP_ID, INVOICE)).rejects.toThrow(
      "no paymaster sponsors",
    );
  });
});

const SWAPS_SELECTOR = "0xeb84e7f2";
const IS_ACTIVE_SELECTOR = "0x160c723c";
const TRUE_WORD = `0x${"0".repeat(63)}1`;
const ZERO_WORD = `0x${"0".repeat(64)}`;
const stateWord = (state: number) =>
  `0x${state.toString(16).padStart(64, "0")}`;

describe("retrySwapWithInvoiceWithSigner binds the signed lock to this swap's", () => {
  // Lock B: another active lock of the same depositor the server can claim.
  const OTHER = {
    preimage_hash: `0x${"5b".repeat(32)}`,
    amount: "990000",
    timelock: TIMELOCK + 99,
  };

  it("refuses params naming another lock before the wallet signs", async () => {
    const fetchMock = serverAnswering({
      rehashParams: rehashParams(OTHER),
      rehashSignature: rehashSignature(OTHER),
    });
    const signed: unknown[] = [];
    const sent: unknown[] = [];
    const signer = signerWith({
      signMessage: async (message) => {
        signed.push(message);
        return `0x${"ab".repeat(65)}`;
      },
      sendTransaction: async (tx) => {
        sent.push(tx);
        return TX;
      },
    });
    const client = await clientWith();

    await expect(
      client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer),
    ).rejects.toThrow("another lock than this swap's own");
    expect(signed).toHaveLength(0);
    expect(sent).toHaveLength(0);
    const urls = fetchMock.mock.calls.map(([input]) =>
      input instanceof Request ? input.url : String(input),
    );
    expect(urls.some((url) => /rehash-evm$/.test(url))).toBe(false);
  });

  it.each([
    ["preimage hash", { preimage_hash: OTHER.preimage_hash }],
    ["amount", { amount: OTHER.amount }],
    ["token", { token: USDC }],
    ["sender", { sender: SERVER }],
    ["claim address", { claim_address: SIGNER_ADDRESS }],
    ["timelock", { timelock: OTHER.timelock }],
    ["chain", { chain_id: 1 }],
  ])("refuses a signature whose %s is another lock's", async (field, override) => {
    // ensureRehashMatchesRequest compares the hash to the params, so the
    // params name it too there; everything else comes only in the signature.
    serverAnswering({
      rehashParams:
        field === "preimage hash" ? rehashParams(override) : rehashParams(),
      rehashSignature: rehashSignature(override),
    });
    const sent: unknown[] = [];
    const signer = signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async (tx) => {
        sent.push(tx);
        return TX;
      },
    });
    const storage = new InMemorySwapStorage();
    await storage.store(storedSwap({ direction: "evm_to_lightning" }));
    const client = await Client.builder().withSwapStorage(storage).build();

    await expect(
      client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer),
    ).rejects.toThrow(field);
    expect(sent).toHaveLength(0);
    expect((await storage.get(SWAP_ID))?.evmRehashes).toBeUndefined();
  });

  it("refuses a lock the funding tx created from another sender", async () => {
    // A v6 lock, created by the coordinator: the depositor cannot rehash it.
    serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    const signer = signerWith({
      logs: [REAL_LOG],
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async () => TX,
    });
    const client = await clientWith();

    await expect(
      client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer),
    ).rejects.toThrow("carries no SwapCreated log");
  });

  it("refuses a lock that is no longer active", async () => {
    serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    const sent: unknown[] = [];
    const signer = signerWith({
      active: false,
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async (tx) => {
        sent.push(tx);
        return TX;
      },
    });
    const client = await clientWith();

    await expect(
      client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer),
    ).rejects.toThrow("no longer active");
    expect(sent).toHaveLength(0);
  });

  it("moves the lock a prior local rehash left, not the original", async () => {
    // The lock was created under FIRST_HASH and this client moved it to
    // HASH_LOCK; the server now (honestly) names the HASH_LOCK terms.
    const FIRST_HASH = `0x${"3f".repeat(32)}`;
    const PRIOR_TX = `0x${"3e".repeat(32)}`;
    const created = logFrom(SIGNER_ADDRESS);
    created.topics = [
      created.topics[0],
      FIRST_HASH,
      ...created.topics.slice(2),
    ];
    const key0 = decodeSwapCreatedLog(created)?.key ?? "";
    const receipts: Record<string, (typeof created)[]> = {
      [FUND_TXID]: [created],
      [PRIOR_TX]: [
        {
          address: HTLC,
          topics: [
            SWAP_REHASHED_TOPIC,
            FIRST_HASH,
            key0,
            `0x${"76".repeat(32)}`,
          ],
          data: HASH_LOCK,
        },
      ],
    };
    serverAnswering({
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    const sent: { to: string; data: string }[] = [];
    const signer = signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async (tx) => {
        sent.push(tx);
        return TX;
      },
      waitForReceipt: async (hash) => ({
        status: "success",
        blockNumber: 1n,
        transactionHash: hash,
        logs: receipts[hash] ?? [],
      }),
    });
    const prior = {
      txid: PRIOR_TX,
      fromHashLock: FIRST_HASH,
      hashLock: HASH_LOCK,
      timelock: TIMELOCK,
    };
    const storage = new InMemorySwapStorage();
    await storage.store(
      storedSwap({
        direction: "evm_to_lightning",
        lightningPaymentHash: { hash: FIRST_HASH, source: "invoice" },
        evmRehashes: [prior],
      }),
    );
    const client = await Client.builder().withSwapStorage(storage).build();

    await client.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer);

    expect(sent).toHaveLength(1);
    expect(sent[0].data).toContain(HASH_LOCK.slice(2));
    expect(sent[0].data).not.toContain(FIRST_HASH.slice(2));

    // A server naming the original (already moved) lock is refused.
    serverAnswering({
      rehashParams: rehashParams({ preimage_hash: FIRST_HASH }),
      rehashSignature: rehashSignature({ preimage_hash: FIRST_HASH }),
    });
    const again = await Client.builder().withSwapStorage(storage).build();
    await expect(
      again.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer),
    ).rejects.toThrow("another lock than this swap's own");
  });
});

const ATTACKER = "0xa77ac4e7a77ac4e7a77ac4e7a77ac4e7a77ac4e7";
const OTHER_CONTRACT = "0x0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c";
const DEPOSITS_SELECTOR = "0x3d4dff7b";
const hex = (value: string) => value as `0x${string}`;
/** `HTLCErc20.refundFor` of a lock, encoded by viem as the server would. */
const refundForCall = (
  terms: { hash?: string; sender?: string; timelock?: number } = {},
) =>
  encodeFunctionData({
    abi: parseAbi([
      "function refundFor(bytes32,uint256,address,address,address,uint256)",
    ]),
    args: [
      hex(terms.hash ?? PAYMENT_HASH),
      20305n,
      hex(WBTC),
      hex(terms.sender ?? SIGNER_ADDRESS),
      hex(SERVER),
      BigInt(terms.timelock ?? NEW_TIMELOCK),
    ],
  });
const refundToCall = () =>
  encodeFunctionData({
    abi: parseAbi([
      "function refundTo(bytes32,uint256,address,address,uint256)",
    ]),
    args: [
      hex(PAYMENT_HASH),
      20305n,
      hex(WBTC),
      hex(SERVER),
      BigInt(NEW_TIMELOCK),
    ],
  });

describe("a refund never makes another lock the rehash anchor", () => {
  // Lock B: another active lock of the same depositor, funded in B_TX.
  const B_HASH = `0x${"5b".repeat(32)}`;
  const B_TX = `0x${"5c".repeat(32)}`;
  const lockB = () => {
    const log = logFrom(SIGNER_ADDRESS);
    log.topics = [log.topics[0], B_HASH, ...log.topics.slice(2)];
    return log;
  };
  const receipts = () => ({
    [FUND_TXID]: [logFrom(SIGNER_ADDRESS)],
    [B_TX]: [lockB()],
  });
  const chain = () =>
    signerWith({
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async () => TX,
      waitForReceipt: async (hash) => ({
        status: "success",
        blockNumber: 1n,
        transactionHash: hash,
        logs: (receipts() as Record<string, (typeof REAL_LOG)[]>)[hash] ?? [],
      }),
    });

  /** The stored response, rewritten by the server to describe lock B. */
  async function namingB(storage: InMemorySwapStorage) {
    const stored = await storage.get(SWAP_ID);
    if (!stored) throw new Error("swap not stored");
    await storage.update(SWAP_ID, {
      ...stored.response,
      hash_lock: B_HASH,
      evm_fund_txid: B_TX,
    } as typeof stored.response);
  }

  it("keeps the client-held hash as the anchor when the server's copy names lock B", async () => {
    const storage = new InMemorySwapStorage();
    await storage.store(storedSwap({ direction: "evm_to_lightning" }));
    await namingB(storage);
    serverGone();
    const refunding = await Client.builder().withSwapStorage(storage).build();
    const signer = chain();

    // The refund builds for this swap's own lock, whatever the server names.
    const refund = await refunding.refundSwap(SWAP_ID, { signer });
    expect(refund.evmRefundData?.data).toContain(HASH_LOCK.slice(2));
    expect(refund.evmRefundData?.data).not.toContain(B_HASH.slice(2));
    expect((await storage.get(SWAP_ID))?.evmFundTxid).toBe(FUND_TXID);
    expect((await storage.get(SWAP_ID))?.lightningPaymentHash).toEqual({
      hash: HASH_LOCK,
      source: "invoice",
    });

    const sent: unknown[] = [];
    serverAnswering({
      rehashParams: rehashParams({ preimage_hash: B_HASH }),
      rehashSignature: rehashSignature({ preimage_hash: B_HASH }),
    });
    const retrying = await Client.builder().withSwapStorage(storage).build();
    await expect(
      retrying.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, {
        ...signer,
        sendTransaction: async (tx) => {
          sent.push(tx);
          return TX;
        },
      }),
    ).rejects.toThrow("another lock than this swap's own");
    expect(sent).toHaveLength(0);
  });

  it("does not take lock B's receipt as the anchor, whichever source names it", async () => {
    // No funding recorded here; the server names only B's funding tx.
    const storage = new InMemorySwapStorage();
    await storage.store(
      storedSwap({
        direction: "evm_to_lightning",
        evmFundTxid: undefined,
        responseFundTxid: B_TX,
      }),
    );
    serverAnswering({
      swap: {
        ...swapResponse({ direction: "evm_to_lightning" }),
        evm_fund_txid: B_TX,
      },
      rehashParams: rehashParams({ preimage_hash: B_HASH }),
      rehashSignature: rehashSignature({ preimage_hash: B_HASH }),
    });
    const retrying = await Client.builder().withSwapStorage(storage).build();
    await expect(
      retrying.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, chain()),
    ).rejects.toThrow("No funding of swap");
  });

  it("finds this swap's own lock from a funding tx only the server names", async () => {
    // Funded through CCTP settlement or a wallet that sent the calldata
    // itself: this device recorded no txid, the server's monitor did.
    const storage = new InMemorySwapStorage();
    await storage.store(
      storedSwap({ direction: "evm_to_lightning", evmFundTxid: undefined }),
    );
    serverAnswering({
      swap: {
        ...swapResponse({ direction: "evm_to_lightning" }),
        evm_fund_txid: FUND_TXID,
      },
      rehashParams: rehashParams(),
      rehashSignature: rehashSignature(),
    });
    const sent: { to: string; data: string }[] = [];
    const retrying = await Client.builder().withSwapStorage(storage).build();
    await retrying.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, {
      ...chain(),
      sendTransaction: async (tx) => {
        sent.push(tx);
        return TX;
      },
    });
    expect(sent).toHaveLength(1);
    expect(sent[0].data).toContain(HASH_LOCK.slice(2));
  });

  it("still refuses a recovered swap after a refund recorded a funding tx", async () => {
    const storage = new InMemorySwapStorage();
    await storage.store(
      storedSwap({
        direction: "evm_to_lightning",
        evmFundTxid: undefined,
        lightningPaymentHash: undefined,
      }),
    );
    await namingB(storage);
    serverGone();
    const refunding = await Client.builder().withSwapStorage(storage).build();
    const signer = chain();
    await refunding.refundSwap(SWAP_ID, { signer });
    expect((await storage.get(SWAP_ID))?.evmFundTxid).toBe(B_TX);
    expect((await storage.get(SWAP_ID))?.lightningPaymentHash).toBeUndefined();

    serverAnswering({
      rehashParams: rehashParams({ preimage_hash: B_HASH }),
      rehashSignature: rehashSignature({ preimage_hash: B_HASH }),
    });
    const retrying = await Client.builder().withSwapStorage(storage).build();
    await expect(
      retrying.retrySwapWithInvoiceWithSigner(SWAP_ID, INVOICE, signer),
    ).rejects.toThrow("no payment hash recorded");
  });
});

describe("refundSwap after a rehash this client has no record of", () => {
  /**
   * The lock this client computes (under HASH_LOCK) is inactive with
   * `swaps` = `state`; the lock it moved to (under PAYMENT_HASH) is active.
   */
  const withState =
    (state: number, deposit = SIGNER_ADDRESS) =>
    async ({ data }: { data: string }) => {
      if (data.startsWith(IS_ACTIVE_SELECTOR)) {
        return data.includes(PAYMENT_HASH.slice(2)) ? TRUE_WORD : ZERO_WORD;
      }
      if (data.startsWith(SWAPS_SELECTOR)) return stateWord(state);
      if (data.startsWith(DEPOSITS_SELECTOR)) {
        return `0x${deposit.slice(2).padStart(64, "0")}`;
      }
      return "0x";
    };

  it.each([
    ["rehashed", 4],
    ["not open under these terms", 0],
  ])("falls through to the server's calldata for a lock %s", async (_, state) => {
    const fetchMock = serverAnswering({
      calldata: { coordinator_address: HTLC, calldata: refundForCall() },
    });
    const client = await clientWith();

    const result = await client.refundSwap(SWAP_ID, {
      signer: signerWith({ call: withState(state) }),
    });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.to).toBe(HTLC);
    expect(result.evmRefundData?.data).toBe(refundForCall());
    expect(result.evmRefundData?.recipient).toBe(SIGNER_ADDRESS);
    // The timelock is the checked call's, not the server's swap record.
    expect(result.evmRefundData?.timelockExpiry).toBe(NEW_TIMELOCK);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it.each([
    ["redeemed", 2],
    ["refunded", 3],
  ])("still reports a %s lock as settled", async (_, state) => {
    const fetchMock = serverAnswering({
      calldata: { coordinator_address: HTLC, calldata: refundForCall() },
    });
    const client = await clientWith();

    const result = await client.refundSwap(SWAP_ID, {
      signer: signerWith({
        call: async ({ data }) => {
          if (data.startsWith(IS_ACTIVE_SELECTOR)) return ZERO_WORD;
          if (data.startsWith(SWAPS_SELECTOR)) return stateWord(state);
          return "0x";
        },
      }),
    });

    expect(result.success).toBe(false);
    expect(result.message).toMatch(/already redeemed or refunded/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  const hostile: [string, { to: string; data: string }, string][] = [
    [
      "the HTLC's refund overload that pays another destination",
      {
        to: HTLC,
        data: encodeFunctionData({
          abi: parseAbi([
            "function refund(bytes32,uint256,address,address,uint256,address)",
          ]),
          args: [
            hex(PAYMENT_HASH),
            20305n,
            hex(WBTC),
            hex(SERVER),
            BigInt(NEW_TIMELOCK),
            hex(ATTACKER),
          ],
        }),
      },
      "none of the refund calls",
    ],
    [
      "an approve on the token",
      {
        to: WBTC,
        data: encodeFunctionData({
          abi: parseAbi(["function approve(address,uint256)"]),
          args: [hex(ATTACKER), 2n ** 256n - 1n],
        }),
      },
      "none of the refund calls",
    ],
    [
      "a refundFor on an unrelated contract",
      { to: OTHER_CONTRACT, data: refundForCall() },
      "not this swap's HTLC",
    ],
    [
      "a refundFor of another sender's lock",
      { to: HTLC, data: refundForCall({ sender: ATTACKER }) },
      "none of this client's addresses",
    ],
    [
      "a refundFor with trailing bytes",
      { to: HTLC, data: `${refundForCall()}00` },
      "bytes of arguments",
    ],
    [
      "a refundFor of a lock that is not active",
      { to: HTLC, data: refundForCall({ hash: `0x${"5b".repeat(32)}` }) },
      "not active",
    ],
    [
      "a refundTo on a contract other than the recorded coordinator",
      { to: OTHER_CONTRACT, data: refundToCall() },
      "not this swap's coordinator",
    ],
  ];

  it.each(hostile)("refuses %s", async (_, call, reason) => {
    serverAnswering({
      calldata: { coordinator_address: call.to, calldata: call.data },
    });
    const client = await clientWith();

    const result = await client.refundSwap(SWAP_ID, {
      signer: signerWith({ call: withState(4) }),
    });

    expect(result.success).toBe(false);
    expect(result.evmRefundData).toBeUndefined();
    expect(result.message).toContain("Refused the server's refund calldata");
    expect(result.message).toContain(reason);
  });

  it("refuses a refundTo whose deposit belongs to someone else", async () => {
    serverAnswering({
      calldata: { coordinator_address: COORDINATOR, calldata: refundToCall() },
    });
    const client = await clientWith();

    const result = await client.refundSwap(SWAP_ID, {
      signer: signerWith({ call: withState(4, ATTACKER) }),
    });

    expect(result.success).toBe(false);
    expect(result.message).toContain(`would refund the lock to ${ATTACKER}`);
  });

  it.each(
    hostile.filter(([name]) => !name.includes("not active")),
  )("refuses %s without a signer too", async (_, call, reason) => {
    serverAnswering({
      calldata: { coordinator_address: call.to, calldata: call.data },
    });
    const client = await clientWith();

    const result = await client.refundSwap(SWAP_ID);

    expect(result.success).toBe(false);
    expect(result.evmRefundData).toBeUndefined();
    expect(result.message).toContain(reason);
  });

  it("never sends a hostile call through refundEvmWithSigner", async () => {
    serverAnswering({
      calldata: { coordinator_address: HTLC, calldata: hostile[0][1].data },
    });
    const sent: unknown[] = [];
    const client = await clientWith();

    await expect(
      client.refundEvmWithSigner(
        SWAP_ID,
        signerWith({
          call: withState(4),
          sendTransaction: async (tx) => {
            sent.push(tx);
            return TX;
          },
        }),
      ),
    ).rejects.toThrow("Refused the server's refund calldata");
    expect(sent).toHaveLength(0);
  });

  it("refuses a signer on another chain before asking anyone", async () => {
    const fetchMock = serverAnswering({
      calldata: { coordinator_address: HTLC, calldata: refundForCall() },
    });
    const sent: unknown[] = [];
    const client = await clientWith();

    await expect(
      client.refundEvmWithSigner(
        SWAP_ID,
        signerWith({
          chainId: 1,
          call: withState(4),
          sendTransaction: async (tx) => {
            sent.push(tx);
            return TX;
          },
        }),
      ),
    ).rejects.toThrow(/on chain 1 but swap .* on chain 137/);
    expect(sent).toHaveLength(0);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("retrySwapWithInvoice (sponsored)", () => {
  const BUNDLER = "http://bundler.test";
  const USER_OP = `0x${"0e".repeat(32)}`;
  const DEPOSITOR = deriveEvmAddress(new Uint8Array(32).fill(1)).toLowerCase();
  const arbitrumSwap = () => {
    const stored = storedSwap({ direction: "evm_to_lightning" });
    return {
      ...stored,
      response: { ...stored.response, evm_chain_id: 42161 },
    } as typeof stored;
  };
  const params = (overrides: Record<string, unknown> = {}) =>
    rehashParams({ chain_id: 42161, sender: DEPOSITOR, ...overrides });
  const signature = (overrides: Record<string, unknown> = {}) =>
    rehashSignature({ chain_id: 42161, sender: DEPOSITOR, ...overrides });

  /** The server's routes, and an Arbitrum node at the AA RPC. */
  function network(
    routes: { rehashParams: unknown; rehashSignature: unknown },
    lockState: { active: boolean } = { active: true },
    /** UserOp hash -> the tx the bundler reports it included in. */
    included: Record<string, string> = {},
  ) {
    const created = ownLog(DEPOSITOR);
    const rpc = (method: string, args: unknown[]): unknown => {
      switch (method) {
        case "eth_chainId":
          return "0xa4b1";
        case "eth_getUserOperationReceipt": {
          const txid = included[args[0] as string];
          return txid
            ? { success: true, receipt: { transactionHash: txid } }
            : null;
        }
        case "eth_getTransactionByHash":
          return {
            hash: args[0],
            from: DEPOSITOR,
            to: HTLC,
            input: "0x",
            nonce: "0x0",
            value: "0x0",
            gas: "0x5208",
            gasPrice: "0x1",
            blockNumber: "0x1",
            blockHash: `0x${"bb".repeat(32)}`,
            transactionIndex: "0x0",
            type: "0x0",
            v: "0x1b",
            r: `0x${"11".repeat(32)}`,
            s: `0x${"22".repeat(32)}`,
            chainId: "0xa4b1",
          };
        case "eth_getTransactionReceipt":
          return {
            transactionHash: args[0],
            blockNumber: "0x1",
            blockHash: `0x${"bb".repeat(32)}`,
            transactionIndex: "0x0",
            from: DEPOSITOR,
            to: HTLC,
            status: "0x1",
            cumulativeGasUsed: "0x1",
            gasUsed: "0x1",
            effectiveGasPrice: "0x1",
            contractAddress: null,
            logsBloom: `0x${"0".repeat(512)}`,
            type: "0x0",
            logs:
              args[0] === FUND_TXID
                ? [
                    {
                      ...created,
                      blockNumber: "0x1",
                      transactionHash: FUND_TXID,
                      logIndex: "0x0",
                      blockHash: `0x${"bb".repeat(32)}`,
                      transactionIndex: "0x0",
                      removed: false,
                    },
                  ]
                : [],
          };
        case "eth_call": {
          const data =
            (args[0] as { data?: string; input?: string }).data ?? "";
          if (data.startsWith(IS_ACTIVE_SELECTOR)) {
            return lockState.active ? TRUE_WORD : ZERO_WORD;
          }
          // The original lock has not moved while a retry is prepared.
          if (data.startsWith(SWAPS_SELECTOR)) return stateWord(1);
          return "0x";
        }
        default:
          throw new Error(`unexpected rpc ${method}`);
      }
    };
    const fetchMock = vi.fn(
      async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = input instanceof Request ? input.url : String(input);
        if (url.startsWith(BUNDLER)) {
          const raw =
            input instanceof Request
              ? await input.clone().text()
              : String(init?.body);
          const body = JSON.parse(raw);
          const answer = (req: {
            id: number;
            method: string;
            params: unknown[];
          }) => ({
            jsonrpc: "2.0",
            id: req.id,
            result: rpc(req.method, req.params),
          });
          return Response.json(
            Array.isArray(body) ? body.map(answer) : answer(body),
          );
        }
        if (url.includes("rehash-evm/params")) {
          return Response.json(routes.rehashParams);
        }
        if (url.includes("rehash-evm")) {
          return Response.json(routes.rehashSignature);
        }
        throw new Error(`unexpected request ${url}`);
      },
    );
    vi.stubGlobal("fetch", fetchMock);
    return fetchMock;
  }

  const ENTRY_POINT = "0x0000000071727de22e5e9d8baf0edac6f37da032";
  /** A prepared v0.7 UserOp, as the Kernel client's prepare returns it. */
  const PREPARED = {
    sender: DEPOSITOR,
    nonce: 0n,
    callData: "0x",
    callGasLimit: 1n,
    verificationGasLimit: 1n,
    preVerificationGas: 1n,
    maxFeePerGas: 1n,
    maxPriorityFeePerGas: 1n,
  };

  /**
   * A client whose Kernel account client prepares with `prepare`, sends
   * with `send` (eth_sendUserOperation) and waits with `receipt`.
   */
  async function sponsoredClient(
    receipt: () => Promise<unknown> = async () => ({
      success: true,
      receipt: { transactionHash: TX },
    }),
    send: () => Promise<string> = async () => USER_OP,
    storage = new InMemorySwapStorage(),
    prepare: () => Promise<unknown> = async () => PREPARED,
  ) {
    if (!(await storage.get(SWAP_ID))) await storage.store(arbitrumSwap());
    const client = await Client.builder()
      .withSwapStorage(storage)
      .withAa({ bundlerUrl: BUNDLER })
      .build();
    const sendUserOperation = vi.fn(send);
    vi.spyOn(client.cctpInbound, "createSmartAccountClient").mockImplementation(
      async ({ signer }) =>
        ({
          accountAddress: signer.address,
          client: {
            prepareUserOperation: prepare,
            account: {
              signUserOperation: async () => `0x${"5a".repeat(65)}`,
              entryPoint: { address: ENTRY_POINT, version: "0.7" },
            },
            request: async ({ method }: { method: string }) => {
              if (method !== "eth_sendUserOperation") {
                throw new Error(`unexpected ${method}`);
              }
              return sendUserOperation();
            },
            waitForUserOperationReceipt: receipt,
          },
        }) as never,
    );
    return { client, storage, sendUserOperation };
  }

  const key0 = () => decodeSwapCreatedLog(ownLog(DEPOSITOR))?.key ?? "";
  const keyOf = (hash: string) =>
    decodeSwapCreatedLog(ownLog(DEPOSITOR, hash, NEW_TIMELOCK))?.key ?? "";
  /** `SwapRehashed` moving the original lock K0 to `hash`'s key. */
  const movedTo = (hash: string) => ({
    address: HTLC,
    topics: [SWAP_REHASHED_TOPIC, HASH_LOCK, key0(), keyOf(hash)],
    data: hash,
  });
  /**
   * The depositor's wallet on Arbitrum. `receipts` are the txs it can read;
   * `states` answers `swaps(key)`; `active` decides `isActive` by hash.
   */
  const depositorWallet = (chain: {
    receipts: Record<string, (typeof REAL_LOG)[]>;
    states?: Record<string, number>;
    active: (data: string) => boolean;
  }) =>
    baseSignerWith({
      address: DEPOSITOR,
      chainId: 42161,
      signMessage: async () => `0x${"ab".repeat(65)}`,
      sendTransaction: async () => TX,
      waitForReceipt: async (hash) => ({
        status: "success",
        blockNumber: 1n,
        transactionHash: hash,
        logs:
          { [FUND_TXID]: [ownLog(DEPOSITOR)], ...chain.receipts }[hash] ?? [],
      }),
      call: async ({ data }) => {
        if (data.startsWith(SWAPS_SELECTOR)) {
          for (const [key, state] of Object.entries(chain.states ?? {})) {
            if (data.includes(key.slice(2))) return stateWord(state);
          }
          return stateWord(0);
        }
        if (data.startsWith(IS_ACTIVE_SELECTOR)) {
          return chain.active(data) ? TRUE_WORD : ZERO_WORD;
        }
        return "0x";
      },
    });

  afterEach(() => vi.restoreAllMocks());

  it("moves this swap's own lock and records the move", async () => {
    network({ rehashParams: params(), rehashSignature: signature() });
    const { client, storage, sendUserOperation } = await sponsoredClient();

    const result = await client.retrySwapWithInvoice(SWAP_ID, INVOICE);

    expect(result.txHash).toBe(TX);
    expect(sendUserOperation).toHaveBeenCalledTimes(1);
    expect((await storage.get(SWAP_ID))?.evmRehashes).toEqual([
      {
        txid: TX,
        userOpHash: USER_OP,
        fromHashLock: HASH_LOCK,
        hashLock: PAYMENT_HASH,
        timelock: NEW_TIMELOCK,
      },
    ]);
  });

  it("refuses a server signature naming another lock of the depositor", async () => {
    network({
      rehashParams: params(),
      rehashSignature: signature({ amount: "990000" }),
    });
    const { client, storage, sendUserOperation } = await sponsoredClient();

    await expect(client.retrySwapWithInvoice(SWAP_ID, INVOICE)).rejects.toThrow(
      "another lock than this swap's own (amount",
    );
    expect(sendUserOperation).not.toHaveBeenCalled();
    expect((await storage.get(SWAP_ID))?.evmRehashes).toBeUndefined();
  });

  it("refuses params naming another lock before signing", async () => {
    const other = { preimage_hash: `0x${"5b".repeat(32)}` };
    const fetchMock = network({
      rehashParams: params(other),
      rehashSignature: signature(other),
    });
    const { client, sendUserOperation } = await sponsoredClient();

    await expect(client.retrySwapWithInvoice(SWAP_ID, INVOICE)).rejects.toThrow(
      "another lock than this swap's own",
    );
    expect(sendUserOperation).not.toHaveBeenCalled();
    const urls = fetchMock.mock.calls.map(([input]) =>
      input instanceof Request ? input.url : String(input),
    );
    expect(urls.some((url) => /rehash-evm$/.test(url))).toBe(false);
  });

  it("refuses a swap whose payment hash this client never recorded", async () => {
    network({ rehashParams: params(), rehashSignature: signature() });
    const { client, storage, sendUserOperation } = await sponsoredClient();
    const stored = await storage.get(SWAP_ID);
    if (!stored) throw new Error("swap not stored");
    await storage.store({
      ...stored,
      evmFundTxid: undefined,
      lightningPaymentHash: undefined,
    });

    await expect(client.retrySwapWithInvoice(SWAP_ID, INVOICE)).rejects.toThrow(
      "no payment hash recorded",
    );
    expect(sendUserOperation).not.toHaveBeenCalled();
  });

  it("keeps the move on record when the receipt wait gives up after inclusion", async () => {
    network({ rehashParams: params(), rehashSignature: signature() });
    const { client, storage } = await sponsoredClient(async () => {
      throw new Error("WaitForUserOperationReceiptTimeoutError");
    });

    await expect(client.retrySwapWithInvoice(SWAP_ID, INVOICE)).rejects.toThrow(
      "WaitForUserOperationReceiptTimeoutError",
    );
    expect((await storage.get(SWAP_ID))?.evmRehashes).toEqual([
      {
        txid: "",
        userOpHash: USER_OP,
        fromHashLock: HASH_LOCK,
        hashLock: PAYMENT_HASH,
        timelock: NEW_TIMELOCK,
      },
    ]);

    // The UserOp was included in TX: the bundler says so, and TX's receipt
    // links the original key to the moved one.
    network(
      { rehashParams: params(), rehashSignature: signature() },
      undefined,
      {
        [USER_OP]: TX,
      },
    );
    const result = await client.refundSwap(SWAP_ID, {
      signer: depositorWallet({
        receipts: { [TX]: [movedTo(PAYMENT_HASH)] },
        active: (data) => data.includes(PAYMENT_HASH.slice(2)),
      }),
    });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.data).toContain(PAYMENT_HASH.slice(2));
    expect(result.evmRefundData?.timelockExpiry).toBe(NEW_TIMELOCK);
  });

  it("keeps the record when the answer to the send is lost, and follows it once it lands", async () => {
    network({ rehashParams: params(), rehashSignature: signature() });
    const { client, storage, sendUserOperation } = await sponsoredClient(
      undefined,
      async () => {
        throw new Error("fetch failed: socket hang up");
      },
    );

    await expect(client.retrySwapWithInvoice(SWAP_ID, INVOICE)).rejects.toThrow(
      "socket hang up",
    );
    expect(sendUserOperation).toHaveBeenCalledTimes(1);
    const [record] = (await storage.get(SWAP_ID))?.evmRehashes ?? [];
    expect(record?.txid).toBe("");
    // Hashed locally before the send, so the bundler can be asked later.
    expect(record?.userOpHash).toMatch(/^0x[0-9a-f]{64}$/);

    // The bundler had taken it, and it landed.
    network(
      { rehashParams: params(), rehashSignature: signature() },
      undefined,
      {
        [record?.userOpHash ?? ""]: TX,
      },
    );
    const result = await client.refundSwap(SWAP_ID, {
      signer: depositorWallet({
        receipts: { [TX]: [movedTo(PAYMENT_HASH)] },
        active: (data) => data.includes(PAYMENT_HASH.slice(2)),
      }),
    });
    expect(result.success).toBe(true);
    expect(result.evmRefundData?.data).toContain(PAYMENT_HASH.slice(2));
  });

  it("refunds the original lock when a pending move never landed", async () => {
    network({ rehashParams: params(), rehashSignature: signature() });
    const { client, storage } = await sponsoredClient(async () => {
      throw new Error("WaitForUserOperationReceiptTimeoutError");
    });
    await expect(
      client.retrySwapWithInvoice(SWAP_ID, INVOICE),
    ).rejects.toThrow();
    expect((await storage.get(SWAP_ID))?.evmRehashes?.[0]?.txid).toBe("");

    // The bundler knows no receipt for it.
    network({ rehashParams: params(), rehashSignature: signature() });
    const result = await client.refundSwap(SWAP_ID, {
      signer: depositorWallet({
        receipts: {},
        active: (data) => data.includes(HASH_LOCK.slice(2)),
      }),
    });

    expect(result.success).toBe(true);
    expect(result.evmRefundData?.data).toContain(HASH_LOCK.slice(2));
    expect(result.evmRefundData?.data).not.toContain(PAYMENT_HASH.slice(2));
  });

  it("drops the record when the UserOp is refused before it is sent", async () => {
    network({ rehashParams: params(), rehashSignature: signature() });
    const { client, storage, sendUserOperation } = await sponsoredClient(
      undefined,
      undefined,
      undefined,
      async () => {
        throw new Error("paymaster refused the UserOp");
      },
    );

    await expect(client.retrySwapWithInvoice(SWAP_ID, INVOICE)).rejects.toThrow(
      "paymaster refused",
    );
    expect(sendUserOperation).not.toHaveBeenCalled();
    expect((await storage.get(SWAP_ID))?.evmRehashes).toBeUndefined();
  });

  it.each([
    ["active", 1],
    ["redeemed", 2],
  ])("follows the attempt that moved the lock, not one whose destination is squatted (%s)", async (_, squatted) => {
    const SECOND_HASH = `0x${"6a".repeat(32)}`;
    const SECOND_INVOICE = invoiceWithHash(SECOND_HASH);
    expect(invoicePaymentHash(SECOND_INVOICE)).toBe(SECOND_HASH);
    const USER_OP_B = `0x${"0f".repeat(32)}`;
    const TX_B = `0x${"b2".repeat(32)}`;
    const storage = new InMemorySwapStorage();

    // A: the bundler took it, the receipt wait gave up, and it never landed.
    network({ rehashParams: params(), rehashSignature: signature() });
    const first = await sponsoredClient(
      async () => {
        throw new Error("WaitForUserOperationReceiptTimeoutError");
      },
      undefined,
      storage,
    );
    await expect(
      first.client.retrySwapWithInvoice(SWAP_ID, INVOICE),
    ).rejects.toThrow();

    // B: another invoice; the original lock is still open, and B lands.
    network({
      rehashParams: params(),
      rehashSignature: signature({ new_preimage_hash: SECOND_HASH }),
    });
    const second = await sponsoredClient(
      async () => ({ success: true, receipt: { transactionHash: TX_B } }),
      async () => USER_OP_B,
      storage,
    );
    await second.client.retrySwapWithInvoice(SWAP_ID, SECOND_INVOICE);
    expect(
      (await storage.get(SWAP_ID))?.evmRehashes?.map((r) => r.txid),
    ).toEqual(["", TX_B]);

    // On chain: K0 is Rehashed (by B), someone created a lock under A's
    // terms (squatted, `squatted`), and B's key holds the funds.
    const wallet = depositorWallet({
      receipts: { [TX_B]: [movedTo(SECOND_HASH)] },
      states: {
        [key0()]: 4,
        [keyOf(PAYMENT_HASH)]: squatted,
        [keyOf(SECOND_HASH)]: 1,
      },
      active: (data) =>
        data.includes(SECOND_HASH.slice(2)) ||
        (squatted === 1 && data.includes(PAYMENT_HASH.slice(2))),
    });
    // The bundler knows A never landed and B in TX_B.
    network(
      { rehashParams: params(), rehashSignature: signature() },
      undefined,
      { [USER_OP_B]: TX_B },
    );
    serverGone();
    const refunding = await Client.builder()
      .withSwapStorage(storage)
      .withAa({ bundlerUrl: BUNDLER })
      .build();
    const refund = await refunding.refundSwap(SWAP_ID, { signer: wallet });
    expect(refund.success).toBe(true);
    expect(refund.evmRefundData?.data).toContain(SECOND_HASH.slice(2));
    expect(refund.evmRefundData?.data).not.toContain(PAYMENT_HASH.slice(2));

    // And a further retry starts from B's lock.
    const THIRD_HASH = `0x${"7b".repeat(32)}`;
    const moved = { preimage_hash: SECOND_HASH, timelock: NEW_TIMELOCK };
    serverAnswering({
      rehashParams: params({ ...moved, new_timelock: NEW_TIMELOCK + 3600 }),
      rehashSignature: signature({
        ...moved,
        new_preimage_hash: THIRD_HASH,
        new_timelock: NEW_TIMELOCK + 3600,
      }),
    });
    const retrying = await Client.builder()
      .withSwapStorage(storage)
      .withAa({ bundlerUrl: BUNDLER })
      .build();
    const third = await retrying.retrySwapWithInvoiceWithSigner(
      SWAP_ID,
      invoiceWithHash(THIRD_HASH),
      wallet,
    );
    expect(third.newPreimageHash).toBe(THIRD_HASH);
  });
});
