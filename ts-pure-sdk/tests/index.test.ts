import { describe, expect, it } from "vitest";
import { SDK_VERSION } from "../src/index.js";

describe("ts-pure-sdk", () => {
  it("exports a semver SDK_VERSION", () => {
    expect(SDK_VERSION).toMatch(/^\d+\.\d+\.\d+/);
  });
});
