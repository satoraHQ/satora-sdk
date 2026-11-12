import { afterEach, describe, expect, it, vi } from "vitest";
import type { GetSwapResponse } from "../src/api/client.js";
import { computeCoordinatorCallsHash } from "../src/evm/coordinator.js";
import { Client, InMemorySwapStorage } from "../src/index.js";
import type { GaslessSwapResponse } from "../src/redeem/gasless.js";
import { claimViaGasless } from "../src/redeem/gasless.js";
import {
  claimNativeViaRifRelay,
  WalletClaimRequiredError,
} from "../src/redeem/rif-relay.js";
import { claimViaUserOp } from "../src/redeem/userop-claim.js";
import {
  HTLC,
  SERVER,
  SIGNER_ADDRESS,
  SWAP_ID,
  signerWith,
  storedSwap,
  TIMELOCK,
} from "./evm-funding-fixture.js";

vi.mock("../src/redeem/userop-claim.js", async (original) => ({
  ...(await original<typeof import("../src/redeem/userop-claim.js")>()),
  claimViaUserOp: vi.fn(),
}));

vi.mock("../src/redeem/rif-relay.js", async (original) => ({
  ...(await original<typeof import("../src/redeem/rif-relay.js")>()),
  claimNativeViaRifRelay: vi.fn(),
}));

const WBTC = "0x1111111111111111111111111111111111111111";
const directions = [
  "arkade_to_evm",
  "bitcoin_to_evm",
  "lightning_to_evm",
] as const;

async function fixture(
  chainId: number,
  direction: (typeof directions)[number],
  withAa = false,
  withRif = false,
) {
  const swap = {
    id: SWAP_ID,
    direction,
    status: "serverfunded",
    evm_chain_id: chainId,
    evm_htlc_kind: [30, 31].includes(chainId) ? "native" : "erc20",
    evm_htlc_address: HTLC,
    evm_htlc_version: 4,
    evm_coordinator_address: SERVER,
    evm_expected_sats: "10000",
    evm_refund_locktime: TIMELOCK,
    server_evm_address: SERVER,
    target_evm_address: SIGNER_ADDRESS,
    wbtc_address: WBTC,
    target_token: { token_id: WBTC, chain: String(chainId) },
  } as unknown as Extract<
    GetSwapResponse,
    { direction: (typeof directions)[number] }
  >;
  const storage = new InMemorySwapStorage();
  await storage.store(
    storedSwap({ response: swap, secretKey: "01".repeat(32) }),
  );
  const relayed = vi.fn(async () =>
    Response.json({
      id: SWAP_ID,
      status: "clientredeemed",
      tx_hash: "0xrelay",
      message: "relayed",
    }),
  );
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const url = input instanceof Request ? input.url : String(input);
    if (url.includes("chain-config"))
      return Response.json({
        chains: withRif
          ? [
              {
                chain: String(chainId),
                btc_pegged_token: {
                  address: WBTC,
                  decimals: 18,
                  symbol: "RBTC",
                },
                btc_peg_rate: "1",
                rif_relay: { server_url: "https://rif.test" },
              },
            ]
          : [],
      });
    if (url.includes("claim-gasless")) return relayed();
    if (url.includes("redeem-and-swap-calldata"))
      return Response.json({
        min_amount_out: "10000",
        calls_hash: computeCoordinatorCallsHash([]),
      });
    return Response.json(swap);
  });
  vi.stubGlobal("fetch", fetchMock);
  const builder = Client.builder()
    .withBaseUrl("https://example.test")
    .withSwapStorage(storage);
  if (withAa)
    builder.withAa({
      bundlerUrl: "https://bundler.test",
      paymasterPolicyId: "policy",
    });
  return { client: await builder.build(), swap, relayed, fetchMock };
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  vi.resetAllMocks();
});

describe("claim sponsorship stays unchanged outside Rootstock", () => {
  for (const chain of [1, 137, 42161]) {
    it.each(
      directions,
    )(`retains the server relay for %s on chain ${chain} without AA config`, async (direction) => {
      const { client, swap, relayed } = await fixture(chain, direction);
      expect(client.canSponsorClaim(swap)).toBe(true);
      await expect(client.claim(SWAP_ID)).resolves.toMatchObject({
        success: true,
        txHash: "0xrelay",
      });
      expect(relayed).toHaveBeenCalledOnce();
    });
  }

  it("retains configured Arbitrum UserOp sponsorship", async () => {
    const { client, relayed } = await fixture(42161, "arkade_to_evm", true);
    vi.mocked(claimViaUserOp).mockResolvedValue({
      id: SWAP_ID,
      status: "clientredeemed",
      txHash: "0xuserop",
      message: "sponsored",
    });
    await expect(client.claim(SWAP_ID)).resolves.toMatchObject({
      success: true,
      txHash: "0xuserop",
    });
    expect(claimViaUserOp).toHaveBeenCalledOnce();
    expect(relayed).not.toHaveBeenCalled();
  });

  it("retains the plain Arbitrum claim's server fallback when its UserOp fails", async () => {
    const { client, relayed } = await fixture(42161, "bitcoin_to_evm", true);
    vi.mocked(claimViaUserOp).mockRejectedValue(
      new Error("bundler unavailable"),
    );
    vi.spyOn(console, "warn").mockImplementation(() => {});
    await expect(client.claim(SWAP_ID)).resolves.toMatchObject({
      success: true,
      txHash: "0xrelay",
    });
    expect(claimViaUserOp).toHaveBeenCalledOnce();
    expect(relayed).toHaveBeenCalledOnce();
  });

  it.each(
    directions,
  )("requires a wallet for Rootstock %s without contacting the relay", async (direction) => {
    const { client, swap, relayed } = await fixture(30, direction);
    expect(client.canSponsorClaim(swap)).toBe(false);
    await expect(client.claim(SWAP_ID)).rejects.toMatchObject({
      name: "WalletClaimRequiredError",
      message: expect.stringContaining("Rootstock RIF sponsorship"),
    });
    expect(relayed).not.toHaveBeenCalled();
  });

  it.each([
    30, 31,
  ])("preserves configured RIF Relay for wallet-signed claims on chain %s", async (chain) => {
    const { client, relayed } = await fixture(
      chain,
      "arkade_to_evm",
      false,
      true,
    );
    vi.mocked(claimNativeViaRifRelay).mockResolvedValue({
      id: SWAP_ID,
      status: "clientredeemed",
      txHash: "0xrif",
      message: "relayed",
    });
    await expect(client.claim(SWAP_ID)).rejects.toBeInstanceOf(
      WalletClaimRequiredError,
    );
    await expect(
      client.claim(SWAP_ID, {
        signer: signerWith({ chainId: chain }),
      }),
    ).resolves.toMatchObject({ success: true, txHash: "0xrif" });
    expect(claimNativeViaRifRelay).toHaveBeenCalledOnce();
    expect(relayed).not.toHaveBeenCalled();
  });

  it("requires a wallet-paid claim on Rootstock testnet without RIF Relay", async () => {
    const { client, swap, relayed } = await fixture(31, "bitcoin_to_evm");
    expect(client.canSponsorClaim(swap)).toBe(false);
    await expect(client.claim(SWAP_ID)).rejects.toThrow(
      /Rootstock RIF sponsorship/,
    );
    expect(relayed).not.toHaveBeenCalled();
  });

  it("also blocks Rootstock through the low-level SDK relay API", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    await expect(
      claimViaGasless({
        baseUrl: "https://example.test",
        preimage: "00".repeat(32),
        secretKey: new Uint8Array(32).fill(1),
        swap: {
          evm_chain_id: 30,
          evm_expected_sats: "10000",
        } as GaslessSwapResponse,
        destination: SIGNER_ADDRESS,
        minAmountOut: 0n,
        callsHash: `0x${"00".repeat(32)}`,
      }),
    ).rejects.toThrow(/Rootstock RIF sponsorship/);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
