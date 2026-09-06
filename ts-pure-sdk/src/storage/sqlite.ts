/**
 * SQLite storage implementations for Node.js.
 *
 * Uses better-sqlite3 for fast, synchronous SQLite access.
 */

import Database, { type Database as DatabaseType } from "better-sqlite3";
import type { GetSwapResponse } from "../api/client.js";
import type { SwapStorage, WalletStorage } from "./index.js";
import type { StoredSwap } from "./types.js";

/**
 * SQLite-based wallet storage for Node.js.
 *
 * Stores mnemonic and key index in a SQLite database.
 *
 * @example
 * ```ts
 * const storage = new SqliteWalletStorage("./lendaswap.db");
 * const client = await Client.builder()
 *   .withSignerStorage(storage)
 *   .build();
 * ```
 */
export class SqliteWalletStorage implements WalletStorage {
  readonly #db: DatabaseType;

  /**
   * Creates a new SQLite wallet storage.
   * @param dbPath - Path to the SQLite database file. Use ":memory:" for in-memory storage.
   */
  constructor(dbPath: string) {
    this.#db = new Database(dbPath);
    this.#initSchema();
  }

  #initSchema(): void {
    this.#db.exec(`
      CREATE TABLE IF NOT EXISTS wallet (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        mnemonic TEXT,
        key_index INTEGER NOT NULL DEFAULT 0
      );
      INSERT OR IGNORE INTO wallet (id, key_index) VALUES (1, 0);
    `);
  }

  async getMnemonic(): Promise<string | null> {
    const row = this.#db
      .prepare("SELECT mnemonic FROM wallet WHERE id = 1")
      .get() as { mnemonic: string | null } | undefined;
    return row?.mnemonic ?? null;
  }

  async setMnemonic(mnemonic: string): Promise<void> {
    this.#db
      .prepare("UPDATE wallet SET mnemonic = ? WHERE id = 1")
      .run(mnemonic);
  }

  async getKeyIndex(): Promise<number> {
    const row = this.#db
      .prepare("SELECT key_index FROM wallet WHERE id = 1")
      .get() as { key_index: number } | undefined;
    return row?.key_index ?? 0;
  }

  async setKeyIndex(index: number): Promise<void> {
    this.#db.prepare("UPDATE wallet SET key_index = ? WHERE id = 1").run(index);
  }

  async incrementKeyIndex(): Promise<number> {
    const current = await this.getKeyIndex();
    await this.setKeyIndex(current + 1);
    return current;
  }

  async clear(): Promise<void> {
    this.#db
      .prepare("UPDATE wallet SET mnemonic = NULL, key_index = 0 WHERE id = 1")
      .run();
  }

  /**
   * Closes the database connection.
   */
  close(): void {
    this.#db.close();
  }
}

/**
 * SQLite-based swap storage for Node.js.
 *
 * Stores swap data including responses and cryptographic parameters.
 *
 * @example
 * ```ts
 * const storage = new SqliteSwapStorage("./lendaswap.db");
 * const client = await Client.builder()
 *   .withSwapStorage(storage)
 *   .build();
 * ```
 */
export class SqliteSwapStorage implements SwapStorage {
  readonly #db: DatabaseType;

  /**
   * Creates a new SQLite swap storage.
   * @param dbPath - Path to the SQLite database file. Use ":memory:" for in-memory storage.
   */
  constructor(dbPath: string) {
    this.#db = new Database(dbPath);
    this.#initSchema();
  }

  #initSchema(): void {
    this.#db.exec(`
      CREATE TABLE IF NOT EXISTS swaps (
        swap_id TEXT PRIMARY KEY,
        version INTEGER NOT NULL,
        key_index INTEGER NOT NULL,
        response TEXT NOT NULL,
        public_key TEXT NOT NULL,
        preimage TEXT NOT NULL,
        preimage_hash TEXT NOT NULL,
        secret_key TEXT NOT NULL,
        stored_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        evm_fund_txid TEXT,
        evm_coordinator_address TEXT
      );
      CREATE INDEX IF NOT EXISTS idx_swaps_stored_at ON swaps(stored_at);
    `);
    // Databases created before these columns existed.
    for (const column of ["evm_fund_txid", "evm_coordinator_address"]) {
      if (this.#columns().includes(column)) continue;
      try {
        this.#db.exec(`ALTER TABLE swaps ADD COLUMN ${column} TEXT`);
      } catch (error) {
        // Another process may have added it between the check and the ALTER.
        if (!this.#columns().includes(column)) throw error;
      }
    }
  }

  #columns(): string[] {
    return (
      this.#db.prepare("PRAGMA table_info(swaps)").all() as { name: string }[]
    ).map((column) => column.name);
  }

  async get(swapId: string): Promise<StoredSwap | null> {
    const row = this.#db
      .prepare("SELECT * FROM swaps WHERE swap_id = ?")
      .get(swapId) as SqliteSwapRow | undefined;

    if (!row) return null;
    return this.#rowToStoredSwap(row);
  }

  async store(swap: StoredSwap): Promise<void> {
    this.#db
      .prepare(
        `INSERT OR REPLACE INTO swaps
         (swap_id, version, key_index, response, public_key, preimage, preimage_hash, secret_key, stored_at, updated_at, evm_fund_txid, evm_coordinator_address)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      )
      .run(
        swap.swapId,
        swap.version,
        swap.keyIndex,
        JSON.stringify(swap.response),
        swap.publicKey,
        swap.preimage,
        swap.preimageHash,
        swap.secretKey,
        swap.storedAt,
        swap.updatedAt,
        swap.evmFundTxid ?? null,
        swap.evmCoordinatorAddress ?? null,
      );
  }

  async update(swapId: string, response: GetSwapResponse): Promise<void> {
    this.#db
      .prepare(
        "UPDATE swaps SET response = ?, updated_at = ? WHERE swap_id = ?",
      )
      .run(JSON.stringify(response), Date.now(), swapId);
  }

  async delete(swapId: string): Promise<void> {
    this.#db.prepare("DELETE FROM swaps WHERE swap_id = ?").run(swapId);
  }

  async list(): Promise<string[]> {
    const rows = this.#db
      .prepare("SELECT swap_id FROM swaps ORDER BY stored_at DESC")
      .all() as { swap_id: string }[];
    return rows.map((r) => r.swap_id);
  }

  async getAll(): Promise<StoredSwap[]> {
    const rows = this.#db
      .prepare("SELECT * FROM swaps ORDER BY stored_at DESC")
      .all() as SqliteSwapRow[];
    return rows.map((row) => this.#rowToStoredSwap(row));
  }

  async clear(): Promise<void> {
    this.#db.prepare("DELETE FROM swaps").run();
  }

  #rowToStoredSwap(row: SqliteSwapRow): StoredSwap {
    return {
      swapId: row.swap_id,
      version: row.version,
      keyIndex: row.key_index,
      response: JSON.parse(row.response) as GetSwapResponse,
      publicKey: row.public_key,
      preimage: row.preimage,
      preimageHash: row.preimage_hash,
      secretKey: row.secret_key,
      storedAt: row.stored_at,
      updatedAt: row.updated_at,
      evmFundTxid: row.evm_fund_txid ?? undefined,
      evmCoordinatorAddress: row.evm_coordinator_address ?? undefined,
    };
  }

  /**
   * Closes the database connection.
   */
  close(): void {
    this.#db.close();
  }
}

interface SqliteSwapRow {
  swap_id: string;
  version: number;
  key_index: number;
  response: string;
  public_key: string;
  preimage: string;
  preimage_hash: string;
  secret_key: string;
  stored_at: number;
  updated_at: number;
  evm_fund_txid: string | null;
  evm_coordinator_address: string | null;
}

/**
 * Creates a storage factory that uses SQLite for both wallet and swap storage.
 *
 * @param dbPath - Path to the SQLite database file.
 * @returns A storage factory with shared database connection.
 *
 * @example
 * ```ts
 * const { walletStorage, swapStorage, close } = sqliteStorageFactory("./lendaswap.db");
 * const client = await Client.builder()
 *   .withSignerStorage(walletStorage)
 *   .withSwapStorage(swapStorage)
 *   .build();
 *
 * // When done:
 * close();
 * ```
 */
export function sqliteStorageFactory(dbPath: string): {
  walletStorage: SqliteWalletStorage;
  swapStorage: SqliteSwapStorage;
  close: () => void;
} {
  const walletStorage = new SqliteWalletStorage(dbPath);
  const swapStorage = new SqliteSwapStorage(dbPath);

  return {
    walletStorage,
    swapStorage,
    close: () => {
      walletStorage.close();
      swapStorage.close();
    },
  };
}
