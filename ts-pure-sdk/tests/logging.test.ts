import { describe, expect, it } from "vitest";
import {
  createSdkLogger,
  type LogRecord,
  redactLogValue,
} from "../src/logging.js";

describe("logging", () => {
  it("is silent by default", () => {
    const records: LogRecord[] = [];
    const logger = createSdkLogger({
      logger: { info: (record) => records.push(record) },
    });

    logger.info({ message: "hello" });

    expect(records).toEqual([]);
  });

  it("filters by level", () => {
    const records: LogRecord[] = [];
    const logger = createSdkLogger({
      logLevel: "warn",
      logger: {
        info: (record) => records.push(record),
        warn: (record) => records.push(record),
      },
    });

    logger.info({ message: "ignored" });
    logger.warn({ message: "emitted" });

    expect(records.map((record) => record.message)).toEqual(["emitted"]);
  });

  it("redacts secret fields", () => {
    expect(
      redactLogValue({
        userSecretKey: "secret",
        nested: { preimage: "secret", txid: "public" },
      }),
    ).toEqual({
      userSecretKey: "[REDACTED]",
      nested: { preimage: "[REDACTED]", txid: "public" },
    });
  });
});
