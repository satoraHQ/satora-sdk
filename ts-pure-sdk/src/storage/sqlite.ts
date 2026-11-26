/**
 * SQLite storage implementations for Node.js.
 *
 * Uses better-sqlite3 for fast, synchronous SQLite access.
 */

import Database, { type Database as DatabaseType } from "better-sqlite3";
import type { GetSwapResponse } from "../api/client.js";
import type { EvmFundingTerms } from "../evm/funding-pins.js";
import type { SwapStorage, WalletStorage } from "./index.js";
import type {
  BitcoinPayout,
  EvmRehashRecord,
  LightningPaymentHash,
  StoredSwap,
} from "./types.js";

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
    // Reserve the index in one statement, including across connections to
    // the same database. A read followed by an awaited write can hand two
    // swaps the same key (and therefore the same deposit address).
    const row = this.#db
      .prepare(
        "UPDATE wallet SET key_index = key_index + 1 WHERE id = 1 RETURNING key_index - 1 AS reserved_index",
      )
      .get() as { reserved_index: number } | undefined;
    if (!row) throw new Error("Cannot reserve a swap key: wallet row missing");
    return row.reserved_index;
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
        evm_coordinator_address TEXT,
        cctp_burn_txid TEXT,
        cctp_burn_last_valid_block_height INTEGER,
        evm_rehashes TEXT,
        lightning_payment_hash TEXT,
        evm_funding_terms TEXT,
        target_address TEXT,
        bridge_recipient TEXT,
        bridge_recipient_wallet TEXT
      );
      CREATE INDEX IF NOT EXISTS idx_swaps_stored_at ON swaps(stored_at);
    `);
    // Databases created before these columns existed.
    for (const [column, type] of [
      ["bitcoin_payout", "TEXT"],
      ["evm_fund_txid", "TEXT"],
      ["evm_funding_user_op_hash", "TEXT"],
      ["evm_coordinator_address", "TEXT"],
      ["cctp_burn_txid", "TEXT"],
      ["cctp_burn_last_valid_block_height", "INTEGER"],
      ["evm_rehashes", "TEXT"],
      ["lightning_payment_hash", "TEXT"],
      ["evm_funding_terms", "TEXT"],
      ["target_address", "TEXT"],
      ["bridge_recipient", "TEXT"],
      ["bridge_recipient_wallet", "TEXT"],
      ["pay_recipient_names", "TEXT"],
      ["pay_requested_amounts", "TEXT"],
      ["pay_usd_rate", "REAL"],
      ["evm_depositor_key_index", "INTEGER"],
      ["bitcoin_claim_broadcast", "TEXT"],
      ["evm_depositor_secret_key", "TEXT"],
      ["superseded_by", "TEXT"],
    ]) {
      if (this.#columns().includes(column)) continue;
      try {
        this.#db.exec(`ALTER TABLE swaps ADD COLUMN ${column} ${type}`);
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
         (swap_id, version, key_index, response, public_key, preimage, preimage_hash, secret_key, stored_at, updated_at, evm_fund_txid, evm_coordinator_address, cctp_burn_txid, cctp_burn_last_valid_block_height, evm_rehashes, lightning_payment_hash, evm_funding_terms, target_address, bridge_recipient, bridge_recipient_wallet, evm_funding_user_op_hash, bitcoin_payout, pay_recipient_names, pay_usd_rate, evm_depositor_key_index, bitcoin_claim_broadcast, evm_depositor_secret_key, pay_requested_amounts, superseded_by)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
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
        swap.cctpBurnTxid ?? null,
        swap.cctpBurnLastValidBlockHeight ?? null,
        swap.evmRehashes ? JSON.stringify(swap.evmRehashes) : null,
        swap.lightningPaymentHash
          ? JSON.stringify(swap.lightningPaymentHash)
          : null,
        swap.evmFundingTerms ? JSON.stringify(swap.evmFundingTerms) : null,
        swap.targetAddress ?? null,
        swap.bridgeRecipient ?? null,
        swap.bridgeRecipientWallet ?? null,
        swap.evmFundingUserOpHash ?? null,
        swap.bitcoinPayout !== undefined
          ? JSON.stringify(swap.bitcoinPayout)
          : null,
        swap.payRecipientNames ? JSON.stringify(swap.payRecipientNames) : null,
        swap.payUsdRate ?? null,
        swap.evmDepositorKeyIndex ?? null,
        swap.bitcoinClaimBroadcast ?? null,
        swap.evmDepositorSecretKey ?? null,
        swap.payRequestedAmounts
          ? JSON.stringify(swap.payRequestedAmounts)
          : null,
        swap.supersededBy ?? null,
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
      evmFundingUserOpHash: row.evm_funding_user_op_hash ?? undefined,
      bitcoinPayout: row.bitcoin_payout
        ? (JSON.parse(row.bitcoin_payout) as BitcoinPayout)
        : undefined,
      evmCoordinatorAddress: row.evm_coordinator_address ?? undefined,
      cctpBurnTxid: row.cctp_burn_txid ?? undefined,
      cctpBurnLastValidBlockHeight:
        row.cctp_burn_last_valid_block_height ?? undefined,
      evmRehashes: row.evm_rehashes
        ? (JSON.parse(row.evm_rehashes) as EvmRehashRecord[])
        : undefined,
      lightningPaymentHash: row.lightning_payment_hash
        ? (JSON.parse(row.lightning_payment_hash) as LightningPaymentHash)
        : undefined,
      evmFundingTerms: row.evm_funding_terms
        ? (JSON.parse(row.evm_funding_terms) as EvmFundingTerms)
        : undefined,
      targetAddress: row.target_address ?? undefined,
      bridgeRecipient: row.bridge_recipient ?? undefined,
      bridgeRecipientWallet: row.bridge_recipient_wallet ?? undefined,
      payRecipientNames: row.pay_recipient_names
        ? JSON.parse(row.pay_recipient_names)
        : undefined,
      payRequestedAmounts: row.pay_requested_amounts
        ? JSON.parse(row.pay_requested_amounts)
        : undefined,
      payUsdRate: row.pay_usd_rate ?? undefined,
      evmDepositorKeyIndex: row.evm_depositor_key_index ?? undefined,
      bitcoinClaimBroadcast: row.bitcoin_claim_broadcast ?? undefined,
      evmDepositorSecretKey: row.evm_depositor_secret_key ?? undefined,
      supersededBy: row.superseded_by ?? undefined,
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
  evm_depositor_key_index: number | null;
  bitcoin_claim_broadcast: string | null;
  evm_depositor_secret_key: string | null;
  superseded_by: string | null;
  bitcoin_payout: string | null;
  pay_recipient_names: string | null;
  pay_requested_amounts: string | null;
  pay_usd_rate: number | null;
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
  evm_funding_user_op_hash: string | null;
  evm_coordinator_address: string | null;
  cctp_burn_txid: string | null;
  cctp_burn_last_valid_block_height: number | null;
  evm_rehashes: string | null;
  lightning_payment_hash: string | null;
  evm_funding_terms: string | null;
  target_address: string | null;
  bridge_recipient: string | null;
  bridge_recipient_wallet: string | null;
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
