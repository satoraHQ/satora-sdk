import { describe, expect, it } from "vitest";
import { SDK_COMMIT_HASH, SDK_VERSION } from "../src/index.js";

describe("ts-pure-sdk", () => {
  it("exports a semver SDK_VERSION", () => {
    expect(SDK_VERSION).toMatch(/^\d+\.\d+\.\d+/);
  });

  it("exports a SDK_COMMIT_HASH string", () => {
    expect(typeof SDK_COMMIT_HASH).toBe("string");
    expect(SDK_COMMIT_HASH.length).toBeGreaterThan(0);
  });
});
