import { describe, expect, it } from "vitest";
import { createApiClient } from "../src/index.js";

describe("API Client", () => {
  it("should create a client with base URL", () => {
    const client = createApiClient({
      baseUrl: "https://api.lendaswap.com",
    });

    expect(client).toBeDefined();
    expect(client.GET).toBeDefined();
    expect(client.POST).toBeDefined();
  });

  it("should create a client with default headers", () => {
    const client = createApiClient({
      baseUrl: "https://api.lendaswap.com",
      defaultHeaders: { "X-Client-Id": "test-client-id" },
    });

    expect(client).toBeDefined();
  });
});

describe("API Client - Type Safety", () => {
  it("should have typed GET methods for known endpoints", async () => {
    const client = createApiClient({
      baseUrl: "https://api.lendaswap.com",
    });

    // These should type-check correctly
    // We're not actually calling the API, just verifying types compile
    const _getTokens = () => client.GET("/tokens");
    const _getQuote = () =>
      client.GET("/quote", {
        params: {
          query: {
            source_chain: "Arkade",
            source_token: "btc",
            target_chain: "137",
            target_token: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359",
            source_amount: 100000,
          },
        },
      });
    const _getSwap = () =>
      client.GET("/swap/{id}", {
        params: { path: { id: "123e4567-e89b-12d3-a456-426614174000" } },
      });

    expect(_getTokens).toBeDefined();
    expect(_getQuote).toBeDefined();
    expect(_getSwap).toBeDefined();
  });

  it("should have typed POST methods for swap creation", async () => {
    const client = createApiClient({
      baseUrl: "https://api.lendaswap.com",
    });

    // Verify POST methods exist and type-check
    const evmBody = {
      target_address: "0x1234567890123456789012345678901234567890",
      claiming_address: "0x1234567890123456789012345678901234567890",
      token_address: "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359",
      evm_chain_id: 137,
      hash_lock: `0x${"ab".repeat(32)}`,
      refund_pk: `02${"cd".repeat(32)}`,
      user_id: `03${"ef".repeat(32)}`,
      amount_in: 100000,
    };

    const _createArkadeToPolygon = () =>
      client.POST("/swap/arkade/evm", {
        body: evmBody,
      });

    const _createLightningToPolygon = () =>
      client.POST("/swap/lightning/evm", {
        body: evmBody,
      });

    const _createBitcoinToPolygon = () =>
      client.POST("/swap/bitcoin/evm", {
        body: evmBody,
      });

    expect(_createArkadeToPolygon).toBeDefined();
    expect(_createLightningToPolygon).toBeDefined();
    expect(_createBitcoinToPolygon).toBeDefined();
  });
});
