import { HDKey } from "@scure/bip32";
import * as bip39 from "@scure/bip39";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { Client, ClientBuilder, InMemoryWalletStorage } from "../src/index.js";

const TEST_MNEMONIC =
  "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

const healthyStatus = {
  healthy: true,
  services: {
    arbitrum: { healthy: true },
    arkade: { healthy: true },
    bitcoin: { healthy: true },
    ethereum: { healthy: true },
    lightning: { healthy: true },
    polygon: { healthy: true },
  },
};

function xprvFor(mnemonic: string): string {
  const seed = bip39.mnemonicToSeedSync(mnemonic, "");
  return HDKey.fromMasterSeed(seed).privateExtendedKey;
}

describe("Client", () => {
  it("should create a client with builder", async () => {
    const client = await Client.builder().build();

    expect(client).toBeDefined();
    expect(client.baseUrl).toBe("https://api.lendaswap.com");
  });

  it("should expose the underlying API client", async () => {
    const client = await Client.builder().build();

    expect(client.api).toBeDefined();
    expect(client.api.GET).toBeDefined();
    expect(client.api.POST).toBeDefined();
  });

  it("should have convenience methods", async () => {
    const client = await Client.builder().build();

    expect(client.getStatus).toBeDefined();
    expect(client.healthCheck).toBeDefined();
    expect(client.getVersion).toBeDefined();
    expect(client.getTokens).toBeDefined();
    expect(client.getQuote).toBeDefined();
    expect(client.getSwap).toBeDefined();
  });

  it("should get detailed API status", async () => {
    const fetchMock = vi.fn(
      async (_input: RequestInfo | URL, _init?: RequestInit) =>
        Response.json(healthyStatus, { status: 200 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const client = await Client.builder()
      .withBaseUrl("https://example.test")
      .build();

    await expect(client.getStatus()).resolves.toEqual(healthyStatus);
    expect(fetchMock).toHaveBeenCalledOnce();
    const [request] = fetchMock.mock.calls[0] ?? [];
    expect(request).toBeInstanceOf(Request);
    expect((request as Request).url).toBe("https://example.test/status");

    vi.unstubAllGlobals();
  });

  it("should return detailed API status for unhealthy dependencies", async () => {
    const unhealthyStatus = {
      ...healthyStatus,
      healthy: false,
      services: {
        ...healthyStatus.services,
        bitcoin: { healthy: false, error: "unavailable" },
      },
    };
    const fetchMock = vi.fn(
      async (_input: RequestInfo | URL, _init?: RequestInit) =>
        Response.json(unhealthyStatus, { status: 503 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const client = await Client.builder()
      .withBaseUrl("https://example.test")
      .build();

    await expect(client.getStatus()).resolves.toEqual(unhealthyStatus);

    vi.unstubAllGlobals();
  });
});

describe("ClientBuilder", () => {
  it("should build a client with default base URL", async () => {
    const client = await Client.builder().build();

    expect(client).toBeDefined();
    expect(client.baseUrl).toBe("https://api.lendaswap.com");
  });

  it("should build a client with custom base URL", async () => {
    const client = await Client.builder()
      .withBaseUrl("https://custom.api.com")
      .build();

    expect(client.baseUrl).toBe("https://custom.api.com");
  });

  it("should support method chaining", async () => {
    const client = await Client.builder()
      .withBaseUrl("https://custom.api.com")
      .withDefaultHeaders({ "X-Client-Id": "test-client-id" })
      .build();

    expect(client).toBeDefined();
    expect(client.baseUrl).toBe("https://custom.api.com");
  });

  it("should build a client with default headers", async () => {
    const client = await Client.builder()
      .withDefaultHeaders({ "X-Client-Id": "test-client-id" })
      .build();

    expect(client).toBeDefined();
  });

  it("should create new builder from ClientBuilder class", async () => {
    const builder = new ClientBuilder();
    const client = await builder.withBaseUrl("https://test.api.com").build();

    expect(client.baseUrl).toBe("https://test.api.com");
  });

  it("should build a client with signer storage", async () => {
    const storage = new InMemoryWalletStorage();
    const client = await Client.builder().withSignerStorage(storage).build();

    expect(client).toBeDefined();
  });
});

describe("Client Signer", () => {
  let storage: InMemoryWalletStorage;

  beforeEach(() => {
    storage = new InMemoryWalletStorage();
  });

  it("should generate mnemonic on build", async () => {
    const client = await Client.builder().withSignerStorage(storage).build();

    const mnemonic = client.getMnemonic();
    expect(mnemonic.split(" ")).toHaveLength(12);
  });

  it("should persist mnemonic to storage", async () => {
    const client = await Client.builder().withSignerStorage(storage).build();
    const mnemonic = client.getMnemonic();

    const storedMnemonic = await storage.getMnemonic();
    expect(storedMnemonic).toBe(mnemonic);
  });

  it("should use provided mnemonic", async () => {
    const mnemonic =
      "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const client = await Client.builder()
      .withSignerStorage(storage)
      .withMnemonic(mnemonic)
      .build();

    expect(client.getMnemonic()).toBe(mnemonic);
  });

  it("should persist provided mnemonic to storage", async () => {
    const mnemonic =
      "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    await Client.builder()
      .withSignerStorage(storage)
      .withMnemonic(mnemonic)
      .build();

    const storedMnemonic = await storage.getMnemonic();
    expect(storedMnemonic).toBe(mnemonic);
  });

  it("should load existing mnemonic from storage", async () => {
    const mnemonic =
      "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    await storage.setMnemonic(mnemonic);

    const client = await Client.builder().withSignerStorage(storage).build();

    expect(client.getMnemonic()).toBe(mnemonic);
  });

  it("should derive swap params and increment key index", async () => {
    const client = await Client.builder().withSignerStorage(storage).build();

    const params1 = await client.deriveSwapParams();
    expect(params1.keyIndex).toBe(0);

    const params2 = await client.deriveSwapParams();
    expect(params2.keyIndex).toBe(1);

    const keyIndex = await client.getKeyIndex();
    expect(keyIndex).toBe(2);
  });

  it("should derive swap params at specific index", async () => {
    const client = await Client.builder().withSignerStorage(storage).build();

    const params = client.deriveSwapParamsAtIndex(5);
    expect(params.keyIndex).toBe(5);

    // Should not affect stored key index
    const keyIndex = await client.getKeyIndex();
    expect(keyIndex).toBe(0);
  });

  it("should work without storage (stateless mode)", async () => {
    const client = await Client.builder().build();

    expect(client.getMnemonic().split(" ")).toHaveLength(12);
  });

  it("should get user ID xpub", async () => {
    const client = await Client.builder().withSignerStorage(storage).build();

    const xpub = client.getUserIdXpub();
    // Should be a base58-encoded extended public key starting with "xpub"
    expect(xpub).toMatch(
      /^xpub[123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz]+$/,
    );
    expect(xpub).toHaveLength(111);
  });

  it("should set key index", async () => {
    const client = await Client.builder().withSignerStorage(storage).build();

    await client.setKeyIndex(10);

    const keyIndex = await client.getKeyIndex();
    expect(keyIndex).toBe(10);
  });

  it("should throw when setting key index without storage", async () => {
    const client = await Client.builder().build();

    await expect(client.setKeyIndex(10)).rejects.toThrow(
      "No signer storage configured",
    );
  });

  it("should throw on invalid mnemonic", async () => {
    await expect(
      Client.builder().withMnemonic("invalid mnemonic").build(),
    ).rejects.toThrow("Invalid mnemonic phrase");
  });

  it("should derive same params for same mnemonic", async () => {
    const mnemonic =
      "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    const client1 = await Client.builder().withMnemonic(mnemonic).build();
    const client2 = await Client.builder().withMnemonic(mnemonic).build();

    const params1 = client1.deriveSwapParamsAtIndex(0);
    const params2 = client2.deriveSwapParamsAtIndex(0);

    expect(params1.keyIndex).toBe(params2.keyIndex);
    expect(params1.preimage).toEqual(params2.preimage);
    expect(params1.preimageHash).toEqual(params2.preimageHash);
  });
});

describe("Client xprv signer", () => {
  it("should build a client from an xprv", async () => {
    const xprv = xprvFor(TEST_MNEMONIC);
    const client = await Client.builder().withXprv(xprv).build();

    expect(client).toBeDefined();
  });

  it("should derive the same params as a mnemonic-based client", async () => {
    const xprv = xprvFor(TEST_MNEMONIC);

    const fromMnemonic = await Client.builder()
      .withMnemonic(TEST_MNEMONIC)
      .build();
    const fromXprv = await Client.builder().withXprv(xprv).build();

    const a = fromMnemonic.deriveSwapParamsAtIndex(0);
    const b = fromXprv.deriveSwapParamsAtIndex(0);

    expect(a.preimage).toEqual(b.preimage);
    expect(a.preimageHash).toEqual(b.preimageHash);
    expect(a.publicKey).toEqual(b.publicKey);
  });

  it("should throw when calling getMnemonic on an xprv-based client", async () => {
    const client = await Client.builder()
      .withXprv(xprvFor(TEST_MNEMONIC))
      .build();

    expect(() => client.getMnemonic()).toThrow(/xprv/);
  });

  it("should not write the secret to signer storage", async () => {
    const storage = new InMemoryWalletStorage();
    await Client.builder()
      .withSignerStorage(storage)
      .withXprv(xprvFor(TEST_MNEMONIC))
      .build();

    expect(await storage.getMnemonic()).toBeNull();
  });

  it("should not load mnemonic from storage when xprv is provided", async () => {
    const storage = new InMemoryWalletStorage();
    // Pre-populate storage with a *different* mnemonic to make sure xprv wins.
    const otherMnemonic =
      "legal winner thank year wave sausage worth useful legal winner thank yellow";
    await storage.setMnemonic(otherMnemonic);

    const client = await Client.builder()
      .withSignerStorage(storage)
      .withXprv(xprvFor(TEST_MNEMONIC))
      .build();

    // Storage value untouched
    expect(await storage.getMnemonic()).toBe(otherMnemonic);

    // Derived params come from the xprv, not the stored mnemonic
    const xprvParams = client.deriveSwapParamsAtIndex(0);
    const referenceClient = await Client.builder()
      .withMnemonic(TEST_MNEMONIC)
      .build();
    const referenceParams = referenceClient.deriveSwapParamsAtIndex(0);
    expect(xprvParams.preimage).toEqual(referenceParams.preimage);
  });

  it("should still use storage for the key index counter", async () => {
    const storage = new InMemoryWalletStorage();
    const client = await Client.builder()
      .withSignerStorage(storage)
      .withXprv(xprvFor(TEST_MNEMONIC))
      .build();

    const p1 = await client.deriveSwapParams();
    const p2 = await client.deriveSwapParams();
    expect(p1.keyIndex).toBe(0);
    expect(p2.keyIndex).toBe(1);
    expect(await storage.getKeyIndex()).toBe(2);
  });

  it("should reject combining withMnemonic and withXprv", async () => {
    await expect(
      Client.builder()
        .withMnemonic(TEST_MNEMONIC)
        .withXprv(xprvFor(TEST_MNEMONIC))
        .build(),
    ).rejects.toThrow(/mutually exclusive/);
  });

  it("should throw on a malformed xprv", async () => {
    await expect(
      Client.builder().withXprv("not-an-xprv").build(),
    ).rejects.toThrow(/Invalid xprv/);
  });

  it("should reject an empty xprv at withXprv() rather than silently falling back", () => {
    expect(() => Client.builder().withXprv("")).toThrow(/non-empty xprv/);
    expect(() => Client.builder().withXprv("   ")).toThrow(/non-empty xprv/);
  });

  it("should not silently generate a wallet when an empty xprv is paired with storage", async () => {
    // The whole point of fail-fast: if env injects "", we must NOT fall through
    // to storage and end up with the wrong keys.
    const storage = new InMemoryWalletStorage();
    expect(() =>
      Client.builder().withSignerStorage(storage).withXprv(""),
    ).toThrow(/non-empty xprv/);
    // And nothing got written to storage as a side effect.
    expect(await storage.getMnemonic()).toBeNull();
  });
});
