import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import Database from "better-sqlite3";
import { afterEach, describe, expect, it } from "vitest";
import { SqliteSwapStorage } from "../src/storage/sqlite.js";
import {
  COORDINATOR,
  FUND_TXID,
  SWAP_ID,
  storedSwap,
} from "./evm-funding-fixture.js";

/** The swaps table as this backend created it before the funding columns. */
const OLD_SCHEMA = `
  CREATE TABLE swaps (
    swap_id TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    key_index INTEGER NOT NULL,
    response TEXT NOT NULL,
    public_key TEXT NOT NULL,
    preimage TEXT NOT NULL,
    preimage_hash TEXT NOT NULL,
    secret_key TEXT NOT NULL,
    stored_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
  )`;

function oldDatabase(presentColumns: string[]): string {
  const path = join(
    mkdtempSync(join(tmpdir(), "lendaswap-sqlite-")),
    "swaps.db",
  );
  const db = new Database(path);
  db.exec(OLD_SCHEMA);
  for (const column of presentColumns) {
    db.exec(`ALTER TABLE swaps ADD COLUMN ${column} TEXT`);
  }
  const legacy = storedSwap({
    evmFundTxid: undefined,
    evmCoordinatorAddress: undefined,
  });
  db.prepare(
    `INSERT INTO swaps
     (swap_id, version, key_index, response, public_key, preimage, preimage_hash, secret_key, stored_at, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    legacy.swapId,
    legacy.version,
    legacy.keyIndex,
    JSON.stringify(legacy.response),
    legacy.publicKey,
    legacy.preimage,
    legacy.preimageHash,
    legacy.secretKey,
    legacy.storedAt,
    legacy.updatedAt,
  );
  db.close();
  return path;
}

describe("SqliteSwapStorage schema migration", () => {
  const paths: string[] = [];

  afterEach(() => {
    for (const path of paths.splice(0)) {
      rmSync(dirname(path), { recursive: true, force: true });
    }
  });

  it.each([
    ["neither column", []],
    ["only evm_fund_txid", ["evm_fund_txid"]],
    ["only evm_coordinator_address", ["evm_coordinator_address"]],
  ])("adds the funding columns to a database created with %s", async (_label, presentColumns) => {
    const path = oldDatabase(presentColumns);
    paths.push(path);

    const storage = new SqliteSwapStorage(path);
    const legacy = await storage.get(SWAP_ID);
    expect(legacy?.evmFundTxid).toBeUndefined();
    expect(legacy?.evmCoordinatorAddress).toBeUndefined();

    await storage.store(storedSwap());
    const funded = await storage.get(SWAP_ID);
    expect(funded?.evmFundTxid).toBe(FUND_TXID);
    expect(funded?.evmCoordinatorAddress).toBe(COORDINATOR);
    storage.close();

    const reopened = new SqliteSwapStorage(path);
    expect((await reopened.get(SWAP_ID))?.evmFundTxid).toBe(FUND_TXID);
    reopened.close();
  });

  it("persists a CCTP-inbound burn, added to a database created before it", async () => {
    // Created with the funding columns but before `cctp_burn_txid`.
    const path = oldDatabase(["evm_fund_txid", "evm_coordinator_address"]);
    paths.push(path);
    const burn = `0x${"b0".repeat(32)}`;

    const storage = new SqliteSwapStorage(path);
    expect((await storage.get(SWAP_ID))?.cctpBurnTxid).toBeUndefined();
    await storage.store(storedSwap({ cctpBurnTxid: burn }));
    // A server refresh rewrites only the response, never the recorded burn.
    const stored = await storage.get(SWAP_ID);
    if (!stored) throw new Error("swap not stored");
    await storage.update(SWAP_ID, stored.response);
    expect((await storage.get(SWAP_ID))?.cctpBurnTxid).toBe(burn);
    storage.close();

    const reopened = new SqliteSwapStorage(path);
    expect((await reopened.get(SWAP_ID))?.cctpBurnTxid).toBe(burn);
    reopened.close();
  });

  it("persists a Solana burn's expiry height, added to a database created before it", async () => {
    // Created with the burn column but before its expiry height.
    const path = oldDatabase([
      "evm_fund_txid",
      "evm_coordinator_address",
      "cctp_burn_txid",
    ]);
    paths.push(path);

    const storage = new SqliteSwapStorage(path);
    expect(
      (await storage.get(SWAP_ID))?.cctpBurnLastValidBlockHeight,
    ).toBeUndefined();
    await storage.store(
      storedSwap({
        cctpBurnTxid: "burn",
        cctpBurnLastValidBlockHeight: 412_345_678,
      }),
    );
    storage.close();

    const reopened = new SqliteSwapStorage(path);
    expect((await reopened.get(SWAP_ID))?.cctpBurnLastValidBlockHeight).toBe(
      412_345_678,
    );
    reopened.close();
  });
});
