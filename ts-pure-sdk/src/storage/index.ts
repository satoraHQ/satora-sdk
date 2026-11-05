/**
 * Storage module for persisting wallet and swap data.
 *
 * This module provides a pluggable storage interface that can be implemented
 * for different backends (IndexedDB, SQLite, etc.).
 */

import type { GetSwapResponse } from "../api/client.js";
import type { StoredSwap } from "./types.js";

export type {
  EvmRehashRecord,
  LightningPaymentHash,
  StoredSwap,
} from "./types.js";
export { SWAP_STORAGE_VERSION } from "./types.js";

/**
 * Storage interface for wallet data (mnemonic, key index).
 *
 * Implementations should handle persistence of sensitive wallet data.
 * For browser environments, consider using IndexedDB with encryption.
 * For desktop/mobile, consider using SQLite with secure storage.
 */
export interface WalletStorage {
  /**
   * Get the stored mnemonic phrase.
   * @returns The mnemonic phrase, or null if not stored.
   */
  getMnemonic(): Promise<string | null>;

  /**
   * Store the mnemonic phrase.
   * @param mnemonic - The mnemonic phrase to store.
   */
  setMnemonic(mnemonic: string): Promise<void>;

  /**
   * Get the current key derivation index.
   * @returns The current key index, defaults to 0 if not set.
   */
  getKeyIndex(): Promise<number>;

  /**
   * Set the key derivation index.
   * @param index - The new key index.
   */
  setKeyIndex(index: number): Promise<void>;

  /**
   * Increment and return the current key index.
   * @returns The index before incrementing (the one to use).
   */
  incrementKeyIndex(): Promise<number>;

  /**
   * Clear all wallet data from storage.
   */
  clear(): Promise<void>;
}

/**
 * Storage interface for swap data.
 *
 * Stores `StoredSwap` records which contain both the API response
 * and client-side parameters needed for claim/refund operations.
 */
export interface SwapStorage {
  /**
   * Get swap data by ID.
   * @param swapId - The swap ID.
   * @returns The stored swap data, or null if not found.
   */
  get(swapId: string): Promise<StoredSwap | null>;

  /**
   * Store a new swap.
   * @param swap - The swap data to store (must include swapId).
   */
  store(swap: StoredSwap): Promise<void>;

  /**
   * Update an existing swap's API response.
   * @param swapId - The swap ID.
   * @param response - The updated API response.
   * @throws Error if swap is not found.
   */
  update(swapId: string, response: GetSwapResponse): Promise<void>;

  /**
   * Delete swap data by ID.
   * @param swapId - The swap ID.
   */
  delete(swapId: string): Promise<void>;

  /**
   * List all swap IDs.
   * @returns Array of swap IDs.
   */
  list(): Promise<string[]>;

  /**
   * Get all stored swaps.
   * @returns Array of all stored swap data.
   */
  getAll(): Promise<StoredSwap[]>;

  /**
   * Clear all swap data from storage.
   */
  clear(): Promise<void>;
}

/**
 * In-memory implementation of WalletStorage.
 *
 * Useful for testing or temporary sessions where persistence is not required.
 * Data is lost when the application is closed.
 *
 * @example
 * ```ts
 * const storage = new InMemoryWalletStorage();
 * await storage.setMnemonic("your mnemonic phrase");
 * ```
 */
export class InMemoryWalletStorage implements WalletStorage {
  #mnemonic: string | null = null;
  #keyIndex = 0;

  async getMnemonic(): Promise<string | null> {
    return this.#mnemonic;
  }

  async setMnemonic(mnemonic: string): Promise<void> {
    this.#mnemonic = mnemonic;
  }

  async getKeyIndex(): Promise<number> {
    return this.#keyIndex;
  }

  async setKeyIndex(index: number): Promise<void> {
    this.#keyIndex = index;
  }

  async incrementKeyIndex(): Promise<number> {
    const current = this.#keyIndex;
    this.#keyIndex = current + 1;
    return current;
  }

  async clear(): Promise<void> {
    this.#mnemonic = null;
    this.#keyIndex = 0;
  }
}

/**
 * In-memory implementation of SwapStorage.
 *
 * Useful for testing or temporary sessions where persistence is not required.
 * Data is lost when the application is closed.
 *
 * @example
 * ```ts
 * const storage = new InMemorySwapStorage();
 * await storage.store({ swapId: "swap-id", ... });
 * ```
 */
export class InMemorySwapStorage implements SwapStorage {
  readonly #data = new Map<string, StoredSwap>();

  async get(swapId: string): Promise<StoredSwap | null> {
    return this.#data.get(swapId) ?? null;
  }

  async store(swap: StoredSwap): Promise<void> {
    this.#data.set(swap.swapId, swap);
  }

  async update(swapId: string, response: GetSwapResponse): Promise<void> {
    const existing = this.#data.get(swapId);
    if (!existing) {
      throw new Error(`Swap not found: ${swapId}`);
    }
    this.#data.set(swapId, {
      ...existing,
      response,
      updatedAt: Date.now(),
    });
  }

  async delete(swapId: string): Promise<void> {
    this.#data.delete(swapId);
  }

  async list(): Promise<string[]> {
    return Array.from(this.#data.keys());
  }

  async getAll(): Promise<StoredSwap[]> {
    return Array.from(this.#data.values());
  }

  async clear(): Promise<void> {
    this.#data.clear();
  }
}

/**
 * Factory function type for creating storage instances.
 *
 * This allows the Client to accept different storage backends.
 */
export type StorageFactory = {
  createWalletStorage: () => WalletStorage;
  createSwapStorage: () => SwapStorage;
};

/**
 * In-memory storage factory.
 *
 * Creates in-memory storage instances for testing or temporary sessions.
 */
export const inMemoryStorageFactory: StorageFactory = {
  createWalletStorage: () => new InMemoryWalletStorage(),
  createSwapStorage: () => new InMemorySwapStorage(),
};
