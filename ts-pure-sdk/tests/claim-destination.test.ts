/**
 * A claim pays out to the target this client sent at creation, never to an
 * address a server answer names: not in the create response, not in a later
 * refresh.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { computeCoordinatorCallsHash } from "../src/evm/coordinator.js";
import {
  Client,
  InMemorySwapStorage,
  InMemoryWalletStorage,
} from "../src/index.js";
import type { StoredSwap } from "../src/storage/types.js";
import {
  assertAddressEchoed,
  SwapVerificationError,
  storedPayoutTarget,
  swapTermsChange,
} from "../src/swap-verification.js";
import { invoiceWithHash } from "./bolt11-fixture.js";

const MNEMONIC =
  "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const USER_DEST = "0x1111111111111111111111111111111111111111";
const ATTACKER_DEST = "0xdeaddeaddeaddeaddeaddeaddeaddeaddeaddead";
const WBTC = "0x2fA66E7BaeC29CE6b6A9D7e0423b1E37A2ad93C6";
const CHAIN_ID = 42161;

type Json = Record<string, unknown>;

let createPatch: Json = {};
let created: Json | undefined;
let refresh: Json | undefined;
let claimBody: Json | undefined;
let calldataDestination: string | null = null;

async function server(
  input: RequestInfo | URL,
  init?: RequestInit,
): Promise<Response> {
  const req = input instanceof Request ? input : new Request(input, init);
  const url = new URL(req.url);
  const path = url.pathname;

  if (req.method === "POST" && path === "/swap/lightning/evm") {
    const body = (await req.json()) as Json;
    created = {
      id: `${String(body.user_id).slice(0, 8)}-0000-4000-8000-000000000000`,
      status: "pending",
      fee_sats: 100,
      hash_lock: body.hash_lock,
      bolt11_invoice: invoiceWithHash(body.hash_lock as string),
      network: "mainnet",
      created_at: Math.floor(Date.now() / 1000),
      chain: "Arbitrum",
      evm_chain_id: CHAIN_ID,
      client_evm_address: body.claiming_address,
      evm_htlc_address: "0xaaa1111111111111111111111111111111111111",
      evm_htlc_version: 7,
      evm_htlc_kind: "erc20",
      evm_coordinator_address: "0xc011d00000000000000000000000000000000001",
      server_evm_address: "0x5e2ee00000000000000000000000000000000001",
      evm_refund_locktime: 1893456000,
      wbtc_address: WBTC,
      evm_expected_sats: "100000",
      source_token: { token_id: "btc", chain: "Bitcoin", decimals: 8 },
      target_token: { token_id: WBTC, chain: "Arbitrum", decimals: 8 },
      target_amount: "100000",
      source_amount: 101000,
      target_evm_address: body.target_address,
      ...createPatch,
    };
    return Response.json(created);
  }
  if (req.method === "GET" && /^\/swap\/[^/]+$/.test(path)) {
    return Response.json(refresh);
  }
  if (req.method === "GET" && path.endsWith("/redeem-and-swap-calldata")) {
    calldataDestination = url.searchParams.get("destination");
    return Response.json({
      calls_hash: computeCoordinatorCallsHash([]),
      min_amount_out: "100000",
    });
  }
  if (req.method === "POST" && path.endsWith("/claim-gasless")) {
    claimBody = (await req.json()) as Json;
    return Response.json({
      id: "x",
      status: "client_redeeming",
      tx_hash: "0xrelayed",
      message: "ok",
    });
  }
  throw new Error(`unexpected ${req.method} ${path}`);
}

/** The server's copy of the created swap, funded, with `patch` applied. */
function refreshed(patch: Json): Json {
  return {
    ...created,
    direction: "lightning_to_evm",
    status: "serverfunded",
    ...patch,
  };
}

async function setup() {
  const storage = new InMemorySwapStorage();
  const client = await Client.builder()
    .withBaseUrl("https://api.test")
    .withMnemonic(MNEMONIC)
    .withSignerStorage(new InMemoryWalletStorage())
    .withSwapStorage(storage)
    .build();
  const create = () =>
    client.createLightningToEvmSwap({
      targetAddress: USER_DEST,
      tokenAddress: WBTC,
      evmChainId: CHAIN_ID,
      sourceAmount: 101000,
    });
  return { client, storage, create };
}

/** Word `n` of the fixed head of `redeemAndExecute` calldata. */
function calldataWord(data: string, n: number): string {
  return data.slice(10 + n * 64, 10 + (n + 1) * 64);
}

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn(server));
  createPatch = {};
  created = undefined;
  refresh = undefined;
  claimBody = undefined;
  calldataDestination = null;
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("creation", () => {
  it("pins the requested target on the stored swap", async () => {
    const { storage, create } = await setup();
    const { response } = await create();
    expect((await storage.get(response.id))?.targetAddress).toBe(USER_DEST);
  });

  it("refuses a target_evm_address other than the one requested", async () => {
    createPatch = { target_evm_address: ATTACKER_DEST };
    const { storage, create } = await setup();
    await expect(create()).rejects.toBeInstanceOf(SwapVerificationError);
    expect(await storage.getAll()).toHaveLength(0);
  });

  it("refuses a client_evm_address other than the claiming key", async () => {
    createPatch = { client_evm_address: ATTACKER_DEST };
    const { storage, create } = await setup();
    await expect(create()).rejects.toBeInstanceOf(SwapVerificationError);
    expect(await storage.getAll()).toHaveLength(0);
  });
});

describe("refresh", () => {
  it("keeps the stored target when a refresh names another", async () => {
    const { client, storage, create } = await setup();
    const { response } = await create();
    refresh = refreshed({ target_evm_address: ATTACKER_DEST });

    const swap = await client.getSwap(response.id, { updateStorage: true });

    expect((swap as Json).target_evm_address).toBe(USER_DEST);
    const stored = await storage.get(response.id);
    expect((stored?.response as Json).target_evm_address).toBe(USER_DEST);
    expect((stored?.response as Json).status).toBe("serverfunded");
  });

  it("refuses a changed payout for EVM-sourced swaps too", () => {
    const stored = {
      swapId: "s",
      response: {
        direction: "evm_to_bitcoin",
        target_btc_address: "bc1quser",
      },
    } as unknown as StoredSwap;
    const fresh = {
      direction: "evm_to_bitcoin",
      target_btc_address: "bc1qattacker",
    } as never;
    expect(swapTermsChange(stored, fresh)?.field).toBe("target_btc_address");
  });
});

describe("a swap without a pinned target", () => {
  const recovered = {
    swapId: "s",
    response: {
      direction: "evm_to_bitcoin",
      target_btc_address: "bc1qreal",
    },
  } as unknown as StoredSwap;

  it("pays the stored copy's field for its direction", () => {
    expect(storedPayoutTarget(recovered)).toBe("bc1qreal");
  });

  it("ignores a payout field of another direction a refresh added", () => {
    const fresh = {
      direction: "evm_to_bitcoin",
      target_btc_address: "bc1qreal",
      target_evm_address: "bc1qattacker",
    };
    expect(swapTermsChange(recovered, fresh as never)).toBeUndefined();
    const refreshed = {
      ...recovered,
      response: fresh,
    } as unknown as StoredSwap;
    expect(storedPayoutTarget(refreshed)).toBe("bc1qreal");
  });

  it("has no target for a Lightning-paid direction", () => {
    const stored = {
      response: {
        direction: "arkade_to_lightning",
        target_evm_address: "0xdead",
      },
    } as unknown as StoredSwap;
    expect(storedPayoutTarget(stored)).toBeUndefined();
  });
});

describe("claims pay the pinned target", () => {
  it("claim() relays a Redeem to the requested target after a poisoned refresh", async () => {
    const { client, create } = await setup();
    const { response } = await create();
    refresh = refreshed({ target_evm_address: ATTACKER_DEST });
    await client.getSwap(response.id, { updateStorage: true });

    const result = await client.claim(response.id);

    expect(result.success).toBe(true);
    expect(calldataDestination?.toLowerCase()).toBe(USER_DEST);
    expect(String(claimBody?.destination).toLowerCase()).toBe(USER_DEST);
  });

  it("claimEvmWithSigner signs to the requested target, whatever the claim-time refresh says", async () => {
    const { client, create } = await setup();
    const { response } = await create();
    refresh = refreshed({ target_evm_address: ATTACKER_DEST });

    const sent: { tx?: { to: string; data: string } } = {};
    const signer = {
      address: "0x2222222222222222222222222222222222222222",
      chainId: CHAIN_ID,
      call: async () => "0x",
      sendTransaction: async (tx: { to: string; data: string }) => {
        sent.tx = tx;
        return "0xfeed";
      },
      waitForReceipt: async (hash: string) => ({
        status: "success",
        blockNumber: 1n,
        transactionHash: hash,
      }),
      getTransaction: async () => undefined,
    };
    await client.claimEvmWithSigner(response.id, signer as never);

    expect(`0x${calldataWord(sent.tx?.data ?? "", 8).slice(24)}`).toBe(
      USER_DEST,
    );
  });
});

describe("assertAddressEchoed", () => {
  const echo = (field: string, value: unknown) => ({ id: "s", [field]: value });

  it("compares hex and bech32 without case, base58 exactly", () => {
    expect(() =>
      assertAddressEchoed(
        echo(
          "target_evm_address",
          "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
        ),
        "target_evm_address",
        "0xaf88d065e77c8cc2239327c5edb3a432268e5831",
      ),
    ).not.toThrow();
    expect(() =>
      assertAddressEchoed(
        echo("target_btc_address", "bc1qxyz"),
        "target_btc_address",
        "BC1QXYZ",
      ),
    ).not.toThrow();
    expect(() =>
      assertAddressEchoed(
        echo("target_evm_address", "TXyzAbc"),
        "target_evm_address",
        "txyzabc",
      ),
    ).toThrow(SwapVerificationError);
  });

  it("refuses a missing echo, and a server-named target nobody asked for", () => {
    expect(() =>
      assertAddressEchoed({ id: "s" }, "target_btc_address", "bc1qxyz"),
    ).toThrow(SwapVerificationError);
    expect(() =>
      assertAddressEchoed(
        echo("target_btc_address", "bc1qattacker"),
        "target_btc_address",
        undefined,
      ),
    ).toThrow(SwapVerificationError);
    expect(() =>
      assertAddressEchoed(
        echo("target_btc_address", null),
        "target_btc_address",
        undefined,
      ),
    ).not.toThrow();
  });
});
