import { describe, expect, it } from "vitest";
import { defaultArkadeServerUrl } from "./arkade-network.js";

describe("defaultArkadeServerUrl", () => {
  it("resolves mainnet/bitcoin to the production server", () => {
    expect(defaultArkadeServerUrl("bitcoin")).toBe("https://arkade.computer");
    expect(defaultArkadeServerUrl("mainnet")).toBe("https://arkade.computer");
  });

  it("resolves signet/mutinynet to the mutinynet server", () => {
    expect(defaultArkadeServerUrl("signet")).toBe(
      "https://mutinynet.arkade.sh",
    );
    expect(defaultArkadeServerUrl("mutinynet")).toBe(
      "https://mutinynet.arkade.sh",
    );
  });

  it("is case-insensitive", () => {
    expect(defaultArkadeServerUrl("Bitcoin")).toBe("https://arkade.computer");
  });

  it("has no default for networks without a public server", () => {
    expect(defaultArkadeServerUrl("regtest")).toBeUndefined();
  });
});
