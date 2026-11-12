import { ripemd160 } from "@noble/hashes/legacy.js";
import { hex } from "@scure/base";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getNetworkHrp } from "../src/arkade-network.js";
import { createArkadeToEvmSwapGeneric } from "../src/create/arkade.js";
import { createBitcoinToEvmSwap } from "../src/create/bitcoin.js";
import { createBitcoinToArkadeSwap } from "../src/create/bitcoin-to-arkade.js";
import type { CreateSwapContext } from "../src/create/types.js";
import { deriveEvmAddress } from "../src/evm/signing.js";
import {
  Client,
  InMemorySwapStorage,
  InMemoryWalletStorage,
  SwapVerificationError,
} from "../src/index.js";
import { parseXOnlyPubKey, refundableVhtlc } from "../src/refund/arkade.js";
import { htlcAddress } from "../src/refund/onchain.js";
import { bytesToHex, type SwapParams } from "../src/signer/index.js";
import vectors from "./fixtures/lightning-invoices.js";

const BASE_URL = "http://swap.test";
const USER_INDEX = 1;
const LOCKTIME = 1_900_000_000;

async function paramsAt(index: number): Promise<SwapParams> {
  const client = await Client.builder().withMnemonic(vectors.mnemonic).build();
  return client.deriveSwapParamsAtIndex(index);
}

const xOnly = (params: SwapParams) =>
  bytesToHex(parseXOnlyPubKey(bytesToHex(params.publicKey)));

/** Keys the server holds: never this wallet's swap keys. */
async function serverKeys() {
  return {
    htlc: xOnly(await paramsAt(90)),
    receiver: xOnly(await paramsAt(91)),
    arkade: xOnly(await paramsAt(92)),
  };
}

function btcHtlc(hash160: string, serverPk: string, user: SwapParams) {
  return htlcAddress(hash160, serverPk, xOnly(user), LOCKTIME, "regtest", 1);
}

function arkadeVhtlc(
  hash160: Uint8Array,
  sender: string,
  receiver: string,
  server: string,
) {
  return refundableVhtlc({
    senderPubKey: sender,
    receiverPubKey: receiver,
    serverPubKey: server,
    preimageHash: hash160,
    refundLocktime: LOCKTIME,
    unilateralClaimDelay: 512,
    unilateralRefundDelay: 1024,
    unilateralRefundWithoutReceiverDelay: 1536,
  })
    .address(getNetworkHrp("regtest"), parseXOnlyPubKey(server))
    .encode();
}

/**
 * Honest server responses for `user`, plus what a server that wants the
 * deposit would put in their place.
 */
async function responses(user: SwapParams) {
  const keys = await serverKeys();
  const hash160 = ripemd160(user.preimageHash);
  const hash160Hex = hex.encode(hash160);
  const attacker = await paramsAt(93);

  const btcToArkade = {
    id: "aaaaaaaa-0000-4000-8000-000000000001",
    network: "regtest",
    hash_lock: hash160Hex,
    server_vhtlc_pk: keys.htlc,
    btc_htlc_address: btcHtlc(hash160Hex, keys.htlc, user),
    btc_htlc_script_version: 1,
    btc_refund_locktime: LOCKTIME,
    arkade_htlc_script_version: 1,
    target_arkade_address:
      vectors.swaps.lightning_to_arkade.response.target_arkade_address,
  };
  // What the server echoes of a to-EVM request: the target and claiming key.
  const evmPayout = {
    target_evm_address: "0x000000000000000000000000000000000000dEaD",
    client_evm_address: deriveEvmAddress(user.secretKey),
  };
  const bitcoinToEvm = {
    id: "aaaaaaaa-0000-4000-8000-000000000002",
    network: "regtest",
    btc_hash_lock: hash160Hex,
    btc_server_pk: keys.htlc,
    btc_htlc_address: btcHtlc(hash160Hex, keys.htlc, user),
    btc_htlc_script_version: 1,
    btc_refund_locktime: LOCKTIME,
    ...evmPayout,
  };
  const arkadeToEvm = {
    id: "aaaaaaaa-0000-4000-8000-000000000003",
    network: "regtest",
    hash_lock: `0x${hex.encode(user.preimageHash)}`,
    sender_pk: xOnly(user),
    receiver_pk: keys.receiver,
    arkade_server_pk: keys.arkade,
    arkade_htlc_script_version: 1,
    vhtlc_refund_locktime: LOCKTIME,
    unilateral_claim_delay: 512,
    unilateral_refund_delay: 1024,
    unilateral_refund_without_receiver_delay: 1536,
    btc_vhtlc_address: arkadeVhtlc(
      hash160,
      xOnly(user),
      keys.receiver,
      keys.arkade,
    ),
    ...evmPayout,
  };

  return {
    btcToArkade,
    bitcoinToEvm,
    arkadeToEvm,
    // A key-path P2TR the server alone can spend.
    attackerAddress: btcHtlc(hash160Hex, keys.htlc, attacker),
    // The same VHTLC with the server, not the user, as its refunding sender.
    serverRefundVhtlc: arkadeVhtlc(
      hash160,
      keys.receiver,
      keys.receiver,
      keys.arkade,
    ),
    otherHash160: hex.encode(ripemd160(attacker.preimageHash)),
  };
}

function contextAnswering(params: SwapParams, response: unknown) {
  const post = vi.fn(async () => ({
    data: response,
    error: undefined,
    response: new Response(null, { status: 200 }),
  }));
  const stored: unknown[] = [];
  const ctx = {
    apiClient: { POST: post } as unknown as CreateSwapContext["apiClient"],
    baseUrl: BASE_URL,
    deriveSwapParams: vi.fn(async () => params),
    evmAddress: "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
    storeSwap: async (...args: unknown[]) => {
      stored.push(args);
    },
  } satisfies CreateSwapContext;
  return { ctx, stored };
}

async function refusal(
  promise: Promise<unknown>,
): Promise<SwapVerificationError> {
  const error = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(SwapVerificationError);
  return error as SwapVerificationError;
}

afterEach(() => vi.unstubAllGlobals());

const evmOptions = {
  targetAddress: "0x000000000000000000000000000000000000dEaD",
  tokenAddress: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
  evmChainId: 42161,
  sourceAmount: 100000n,
};

type Responses = Awaited<ReturnType<typeof responses>>;

const directions = [
  {
    name: "Bitcoin to Arkade",
    honest: (r: Responses) => r.btcToArkade,
    addressField: "btc_htlc_address",
    hashField: "hash_lock",
    versionField: "btc_htlc_script_version",
    stolen: (r: Responses) => ({
      ...r.btcToArkade,
      btc_htlc_address: r.attackerAddress,
    }),
    create: (ctx: CreateSwapContext) =>
      createBitcoinToArkadeSwap(
        {
          targetAddress:
            vectors.swaps.lightning_to_arkade.response.target_arkade_address,
          satsReceive: 100000,
        },
        ctx,
      ),
  },
  {
    name: "Bitcoin to EVM",
    honest: (r: Responses) => r.bitcoinToEvm,
    addressField: "btc_htlc_address",
    hashField: "btc_hash_lock",
    versionField: "btc_htlc_script_version",
    stolen: (r: Responses) => ({
      ...r.bitcoinToEvm,
      btc_htlc_address: r.attackerAddress,
    }),
    create: (ctx: CreateSwapContext) =>
      createBitcoinToEvmSwap(
        { ...evmOptions, sourceAmount: Number(evmOptions.sourceAmount) },
        ctx,
      ),
  },
  {
    name: "Arkade to EVM",
    honest: (r: Responses) => r.arkadeToEvm,
    addressField: "btc_vhtlc_address",
    hashField: "hash_lock",
    versionField: "arkade_htlc_script_version",
    stolen: (r: Responses) => ({
      ...r.arkadeToEvm,
      btc_vhtlc_address: r.serverRefundVhtlc,
    }),
    create: (ctx: CreateSwapContext) =>
      createArkadeToEvmSwapGeneric(evmOptions, ctx),
  },
] as const;

describe.each(directions)("$name creation rebuilds the lock the user funds", ({
  honest,
  stolen,
  create,
  addressField,
  hashField,
  versionField,
}) => {
  it("stores a lock that refunds to this client's key", async () => {
    const user = await paramsAt(USER_INDEX);
    const response = honest(await responses(user));
    const { ctx, stored } = contextAnswering(user, response);
    const result = await create(ctx);
    expect(result.response).toEqual(response);
    expect(stored).toHaveLength(1);
  });

  it("refuses an address the server alone can spend, before storing it", async () => {
    const user = await paramsAt(USER_INDEX);
    const { ctx, stored } = contextAnswering(
      user,
      stolen(await responses(user)),
    );
    const error = await refusal(create(ctx));
    expect(error.field).toBe(addressField);
    expect(stored).toHaveLength(0);
  });

  it("refuses a lock built for another wallet's refund key", async () => {
    const other = await paramsAt(USER_INDEX + 1);
    const user = await paramsAt(USER_INDEX);
    const response = honest(await responses(other));
    const { ctx, stored } = contextAnswering(user, response);
    await refusal(create(ctx));
    expect(stored).toHaveLength(0);
  });

  it("refuses a hash lock that is not the swap's", async () => {
    const user = await paramsAt(USER_INDEX);
    const r = await responses(user);
    const { ctx, stored } = contextAnswering(user, {
      ...honest(r),
      [hashField]: r.otherHash160,
    });
    const error = await refusal(create(ctx));
    expect(error.field).toBe(hashField);
    expect(stored).toHaveLength(0);
  });

  it("refuses a script version this client cannot refund", async () => {
    const user = await paramsAt(USER_INDEX);
    const { ctx, stored } = contextAnswering(user, {
      ...honest(await responses(user)),
      [versionField]: 2,
    });
    const error = await refusal(create(ctx));
    expect(error.field).toBe(versionField);
    expect(stored).toHaveLength(0);
  });
});

describe("recovery checks the lock of a funded-by-user swap it stores for the first time", () => {
  async function recover(swaps: unknown[]) {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const request = new Request(input, init);
        const path = new URL(request.url).pathname;
        if (path === "/swap/recover") {
          return Response.json({
            swaps,
            highest_index: USER_INDEX,
            next_index: USER_INDEX + 1,
            scanned_until: USER_INDEX,
            complete: true,
          });
        }
        throw new Error(`unexpected request ${request.method} ${path}`);
      }),
    );
    const storage = new InMemorySwapStorage();
    const warnings: unknown[] = [];
    const client = await Client.builder()
      .withBaseUrl(BASE_URL)
      .withMnemonic(vectors.mnemonic)
      .withSignerStorage(new InMemoryWalletStorage())
      .withSwapStorage(storage)
      .withLogger({ warn: (record) => warnings.push(record) })
      .withLogLevel("warn")
      .build();
    const result = await client.recoverSwaps();
    return { result, storage, warnings };
  }

  const directionOf = {
    "Bitcoin to Arkade": "btc_to_arkade",
    "Bitcoin to EVM": "bitcoin_to_evm",
    "Arkade to EVM": "arkade_to_evm",
  } as const;

  it("stores honest locks", async () => {
    const r = await responses(await paramsAt(USER_INDEX));
    const { result, warnings } = await recover(
      directions.map((d) => ({
        ...d.honest(r),
        direction: directionOf[d.name],
        index: USER_INDEX,
      })),
    );
    expect(result.swaps.map((swap) => swap.swapId)).toEqual(
      directions.map((d) => d.honest(r).id),
    );
    expect(warnings).toEqual([]);
  });

  it.each(
    directions,
  )("refuses a $name address the server alone can spend", async ({
    name,
    stolen,
    addressField,
  }) => {
    const swap = {
      ...stolen(await responses(await paramsAt(USER_INDEX))),
      direction: directionOf[name],
      index: USER_INDEX,
    };
    const { result, storage, warnings } = await recover([swap]);
    expect(result.swaps).toEqual([]);
    expect(await storage.get(swap.id)).toBeNull();
    expect(warnings).toMatchObject([
      {
        event: "client.recover.terms_refused",
        swapId: swap.id,
        data: { field: addressField },
      },
    ]);
  });
});
