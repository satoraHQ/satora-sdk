import { beforeEach, describe, expect, it } from "vitest";
import {
  BTC_HTLC_SCRIPT_VERSION_STRICT,
  InMemorySwapStorage,
  InMemoryWalletStorage,
  inMemoryStorageFactory,
  type StoredSwap,
  SWAP_STORAGE_VERSION,
} from "../src/index.js";

describe("InMemoryWalletStorage", () => {
  let storage: InMemoryWalletStorage;

  beforeEach(() => {
    storage = new InMemoryWalletStorage();
  });

  describe("mnemonic", () => {
    it("should return null when no mnemonic is set", async () => {
      const mnemonic = await storage.getMnemonic();
      expect(mnemonic).toBeNull();
    });

    it("should store and retrieve mnemonic", async () => {
      const testMnemonic = "test mnemonic phrase";
      await storage.setMnemonic(testMnemonic);
      const retrieved = await storage.getMnemonic();
      expect(retrieved).toBe(testMnemonic);
    });

    it("should overwrite existing mnemonic", async () => {
      await storage.setMnemonic("first mnemonic");
      await storage.setMnemonic("second mnemonic");
      const retrieved = await storage.getMnemonic();
      expect(retrieved).toBe("second mnemonic");
    });
  });

  describe("keyIndex", () => {
    it("should return 0 when no key index is set", async () => {
      const index = await storage.getKeyIndex();
      expect(index).toBe(0);
    });

    it("should store and retrieve key index", async () => {
      await storage.setKeyIndex(42);
      const index = await storage.getKeyIndex();
      expect(index).toBe(42);
    });

    it("should increment key index", async () => {
      await storage.setKeyIndex(5);
      const beforeIncrement = await storage.incrementKeyIndex();
      expect(beforeIncrement).toBe(5);

      const afterIncrement = await storage.getKeyIndex();
      expect(afterIncrement).toBe(6);
    });

    it("should increment from 0 when not set", async () => {
      const index = await storage.incrementKeyIndex();
      expect(index).toBe(0);

      const newIndex = await storage.getKeyIndex();
      expect(newIndex).toBe(1);
    });
  });

  describe("clear", () => {
    it("should clear all data", async () => {
      await storage.setMnemonic("test mnemonic");
      await storage.setKeyIndex(10);

      await storage.clear();

      expect(await storage.getMnemonic()).toBeNull();
      expect(await storage.getKeyIndex()).toBe(0);
    });
  });
});

/** Helper to create a minimal valid stored swap response for tests */
function createTestResponse(swapId: string): StoredSwap["response"] {
  return {
    id: swapId,
    direction: "bitcoin_to_evm",
    evm_htlc_kind: "erc20",
    status: "pending",
    btc_hash_lock: "ab".repeat(20),
    btc_htlc_address: "tb1qtestaddress000000000000000000000000000",
    btc_htlc_script_version: BTC_HTLC_SCRIPT_VERSION_STRICT,
    btc_refund_locktime: 1_700_000_000,
    btc_server_pk: `02${"1".repeat(64)}`,
    chain: "Polygon",
    client_evm_address: "0x1234567890123456789012345678901234567890",
    created_at: new Date(0).toISOString(),
    evm_chain_id: 137,
    evm_coordinator_address: "0x2222222222222222222222222222222222222222",
    evm_expected_sats: "100000",
    evm_hash_lock: `0x${"ab".repeat(32)}`,
    evm_htlc_address: "0x3333333333333333333333333333333333333333",
    evm_htlc_version: 4,
    evm_refund_locktime: 1_700_000_100,
    fee_sats: 100,
    network: "testnet",
    server_evm_address: "0x4444444444444444444444444444444444444444",
    source_amount: "100000",
    source_token: {
      chain: "Bitcoin",
      decimals: 8,
      name: "Bitcoin",
      symbol: "BTC",
      token_id: "btc",
    },
    target_amount: "99000000",
    target_token: {
      chain: "137",
      decimals: 6,
      name: "USD Coin",
      symbol: "USDC",
      token_id: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359",
    },
    wbtc_address: "0x5555555555555555555555555555555555555555",
  };
}

/** Helper to create a test StoredSwap */
function createTestSwap(
  swapId: string,
  overrides: Partial<StoredSwap> = {},
): StoredSwap {
  return {
    version: SWAP_STORAGE_VERSION,
    swapId,
    keyIndex: 0,
    response: createTestResponse(swapId),
    publicKey: `02${"a".repeat(64)}`,
    preimage: "b".repeat(64),
    preimageHash: "c".repeat(64),
    secretKey: "d".repeat(64),
    storedAt: Date.now(),
    updatedAt: Date.now(),
    ...overrides,
  };
}

describe("InMemorySwapStorage", () => {
  let storage: InMemorySwapStorage;

  beforeEach(() => {
    storage = new InMemorySwapStorage();
  });

  describe("get/store", () => {
    it("should return null for non-existent swap", async () => {
      const swap = await storage.get("non-existent");
      expect(swap).toBeNull();
    });

    it("should store and retrieve swap", async () => {
      const testSwap = createTestSwap("swap-1", { keyIndex: 5 });
      await storage.store(testSwap);

      const retrieved = await storage.get("swap-1");
      expect(retrieved).toEqual(testSwap);
    });

    it("should overwrite existing swap", async () => {
      await storage.store(createTestSwap("swap-1", { keyIndex: 1 }));
      await storage.store(createTestSwap("swap-1", { keyIndex: 2 }));

      const retrieved = await storage.get("swap-1");
      expect(retrieved?.keyIndex).toBe(2);
    });
  });

  describe("update", () => {
    it("should update swap response", async () => {
      const testSwap = createTestSwap("swap-1");
      await storage.store(testSwap);

      const updatedResponse = {
        ...testSwap.response,
        status: "clientredeemed",
      } as StoredSwap["response"];
      await storage.update("swap-1", updatedResponse);

      const retrieved = await storage.get("swap-1");
      expect(retrieved?.response.status).toBe("clientredeemed");
      expect(retrieved?.updatedAt).toBeGreaterThanOrEqual(testSwap.updatedAt);
    });

    it("should throw when updating non-existent swap", async () => {
      const response = createTestSwap("swap-1").response;
      await expect(storage.update("non-existent", response)).rejects.toThrow(
        "Swap not found: non-existent",
      );
    });
  });

  describe("delete", () => {
    it("should delete existing swap", async () => {
      await storage.store(createTestSwap("swap-1"));
      await storage.delete("swap-1");

      const retrieved = await storage.get("swap-1");
      expect(retrieved).toBeNull();
    });

    it("should not throw when deleting non-existent swap", async () => {
      await expect(storage.delete("non-existent")).resolves.not.toThrow();
    });
  });

  describe("list", () => {
    it("should return empty array when no swaps", async () => {
      const list = await storage.list();
      expect(list).toEqual([]);
    });

    it("should return all swap IDs", async () => {
      await storage.store(createTestSwap("swap-1"));
      await storage.store(createTestSwap("swap-2"));
      await storage.store(createTestSwap("swap-3"));

      const list = await storage.list();
      expect(list).toHaveLength(3);
      expect(list).toContain("swap-1");
      expect(list).toContain("swap-2");
      expect(list).toContain("swap-3");
    });
  });

  describe("getAll", () => {
    it("should return empty array when no swaps", async () => {
      const all = await storage.getAll();
      expect(all).toEqual([]);
    });

    it("should return all swaps", async () => {
      const swap1 = createTestSwap("swap-1", { keyIndex: 0 });
      const swap2 = createTestSwap("swap-2", { keyIndex: 1 });
      await storage.store(swap1);
      await storage.store(swap2);

      const all = await storage.getAll();
      expect(all).toHaveLength(2);
      expect(all.map((s) => s.swapId)).toContain("swap-1");
      expect(all.map((s) => s.swapId)).toContain("swap-2");
    });
  });

  describe("clear", () => {
    it("should clear all swaps", async () => {
      await storage.store(createTestSwap("swap-1"));
      await storage.store(createTestSwap("swap-2"));

      await storage.clear();

      expect(await storage.list()).toEqual([]);
      expect(await storage.getAll()).toEqual([]);
    });
  });
});

describe("inMemoryStorageFactory", () => {
  it("should create wallet storage", () => {
    const walletStorage = inMemoryStorageFactory.createWalletStorage();
    expect(walletStorage).toBeInstanceOf(InMemoryWalletStorage);
  });

  it("should create swap storage", () => {
    const swapStorage = inMemoryStorageFactory.createSwapStorage();
    expect(swapStorage).toBeInstanceOf(InMemorySwapStorage);
  });
});
