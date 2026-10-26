import {
  type Address,
  decodeFunctionData,
  encodeAbiParameters,
  getTypesForEIP712Domain,
  type Hex,
  hashDomain,
  hashTypedData,
  keccak256,
  parseAbiItem,
  recoverTypedDataAddress,
  toHex,
} from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { describe, expect, it } from "vitest";
import type { EIP712TypedData, EvmSigner } from "../src/evm/wallet.js";
import type { GaslessSwapResponse } from "../src/redeem/gasless.js";
import {
  claimNativeViaRifRelay,
  type RifRelayConfig,
  type RifRequest,
  rifFee,
  rifSmartWalletAddress,
  rifTypedData,
} from "../src/redeem/rif-relay.js";

// Rootstock testnet full-wallet deployment (rif-relay-contracts
// v2.2.1-beta.0, #181): factory and template 2026-10-15, verifiers 2026-10-23.
const RELAY: RifRelayConfig = {
  server_url: "https://relay.test",
  relay_hub: "0xD2EFC2B1e5296235779A79B69e7511764E2FC8cd",
  smart_wallet_factory: "0x85D9C99b8d2fB2f1cc5d54d4e15E3F39e66852B3",
  smart_wallet_template: "0x57dfe76c0cd3d755a6b2a5a4ceedc846cebf4754",
  deploy_verifier: "0x3D52678C7329C75590D35b8415F83c6EDF35BCF8",
  relay_verifier: "0x485C241B07868CE8174e52EFFFe135bCD4E2ADE3",
};
const COORDINATOR = "0x447a4CD711CE66D25c7008d26D5f09cA7D621F48";
const HTLC = "0x18123A31261a76c74C82ED2F8346002CaA78Ab39";
const FEES_RECEIVER = "0x2f13221E896db6EBBA2b2a587bc48d0f8C4162E1";
const ZERO = "0x0000000000000000000000000000000000000000";

const owner = privateKeyToAccount(
  "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
);
/** The owner's smart wallet: the lock's claim address. */
const SMART_WALLET = rifSmartWalletAddress(RELAY, owner.address);
const PREIMAGE = `0x${"11".repeat(32)}`;
const AMOUNT_WEI = 100_000_000_000_000n;

const swap = {
  id: "00000000-0000-0000-0000-000000000031",
  evm_chain_id: 31,
  evm_htlc_kind: "native",
  evm_htlc_address: HTLC,
  evm_coordinator_address: COORDINATOR,
  evm_htlc_version: 1,
  client_evm_address: SMART_WALLET,
  evm_expected_sats: AMOUNT_WEI.toString(),
  evm_refund_locktime: 1_800_000_000,
  server_evm_address: "0xfD97fA3fCE801b67df009d09fA94F3f02147845e",
} as unknown as GaslessSwapResponse;

const RELAY_DATA_STRING =
  "RelayData(uint256 gasPrice,address feesReceiver,address callForwarder,address callVerifier)";
// Verbatim from BaseSmartWalletFactory / BaseSmartWallet `_getEncoded`.
const DEPLOY_TYPE_STRING = `RelayRequest(address relayHub,address from,address to,address tokenContract,address recoverer,uint256 value,uint256 nonce,uint256 tokenAmount,uint256 tokenGas,uint256 validUntilTime,uint256 index,bytes data,RelayData relayData)${RELAY_DATA_STRING}`;
const RELAY_TYPE_STRING = `RelayRequest(address relayHub,address from,address to,address tokenContract,uint256 value,uint256 gas,uint256 nonce,uint256 tokenAmount,uint256 tokenGas,uint256 validUntilTime,bytes data,RelayData relayData)${RELAY_DATA_STRING}`;

const typehash = (s: string) => keccak256(toHex(s));

/** The struct hash the contracts compute, independent of viem's EIP-712. */
function contractStructHash(req: RifRequest): Hex {
  const r = req.request;
  const d = req.relayData;
  const relayDataHash = keccak256(
    encodeAbiParameters(
      [
        { type: "bytes32" },
        { type: "uint256" },
        { type: "address" },
        { type: "address" },
        { type: "address" },
      ],
      [
        typehash(RELAY_DATA_STRING),
        BigInt(d.gasPrice),
        d.feesReceiver as Address,
        d.callForwarder as Address,
        d.callVerifier as Address,
      ],
    ),
  );
  const addr = (k: string) => ({ type: "address", value: r[k] as Address });
  const uint = (k: string) => ({ type: "uint256", value: BigInt(r[k]) });
  const fields =
    req.kind === "deploy"
      ? [
          addr("relayHub"),
          addr("from"),
          addr("to"),
          addr("tokenContract"),
          addr("recoverer"),
          uint("value"),
          uint("nonce"),
          uint("tokenAmount"),
          uint("tokenGas"),
          uint("validUntilTime"),
          uint("index"),
        ]
      : [
          addr("relayHub"),
          addr("from"),
          addr("to"),
          addr("tokenContract"),
          uint("value"),
          uint("gas"),
          uint("nonce"),
          uint("tokenAmount"),
          uint("tokenGas"),
          uint("validUntilTime"),
        ];
  return keccak256(
    encodeAbiParameters(
      [
        { type: "bytes32" },
        ...fields.map((f) => ({ type: f.type })),
        { type: "bytes32" },
        { type: "bytes32" },
      ],
      [
        typehash(
          req.kind === "deploy" ? DEPLOY_TYPE_STRING : RELAY_TYPE_STRING,
        ),
        ...fields.map((f) => f.value),
        keccak256(r.data as Hex),
        relayDataHash,
      ],
    ),
  );
}

/** The domain separator of `td`'s domain. */
function domainSeparatorOf(td: EIP712TypedData): Hex {
  const domain = viemTypedData(td).domain ?? {};
  return hashDomain({
    domain,
    types: { EIP712Domain: getTypesForEIP712Domain({ domain }) },
  } as unknown as Parameters<typeof hashDomain>[0]);
}

function viemTypedData(td: EIP712TypedData) {
  return td as unknown as Parameters<typeof hashTypedData>[0];
}

function request(kind: "deploy" | "relay"): RifRequest {
  const common = {
    relayHub: RELAY.relay_hub,
    from: owner.address,
    to: COORDINATOR,
    tokenContract: ZERO,
    value: "0",
    nonce: "4",
    tokenAmount: "1234",
    tokenGas: "10500",
    validUntilTime: 1_900_000_000,
    data: "0xdeadbeef",
  };
  return kind === "deploy"
    ? {
        kind,
        verifyingContract: RELAY.smart_wallet_factory,
        request: { ...common, recoverer: ZERO, index: 0 },
        relayData: {
          gasPrice: "6352201",
          feesReceiver: FEES_RECEIVER,
          callForwarder: RELAY.smart_wallet_factory,
          callVerifier: RELAY.deploy_verifier,
        },
        relayHub: RELAY.relay_hub,
      }
    : {
        kind,
        verifyingContract: SMART_WALLET,
        request: { ...common, gas: "120000" },
        relayData: {
          gasPrice: "6352201",
          feesReceiver: FEES_RECEIVER,
          callForwarder: SMART_WALLET,
          callVerifier: RELAY.relay_verifier,
        },
        relayHub: RELAY.relay_hub,
      };
}

describe("rifSmartWalletAddress", () => {
  it("matches the factory's getSmartWalletAddress on Rootstock testnet", () => {
    // `factory.getSmartWalletAddress(owner, 0x0, 0)` via eth_call.
    expect(
      rifSmartWalletAddress(
        RELAY,
        "0x1111111111111111111111111111111111111111",
      ),
    ).toBe("0x6109158443ac7fefe6398f087f24aaa960f11dff");
    expect(
      rifSmartWalletAddress(
        RELAY,
        "0xfD97fA3fCE801b67df009d09fA94F3f02147845e",
      ),
    ).toBe("0x95cafafd048ca4643dc1fdeebf35b48ca47930c6");
  });
});

describe("RIF RelayRequest typed data", () => {
  it("uses the factory's on-chain domain separator for a deploy", () => {
    const td = rifTypedData(request("deploy"), 31);
    // factory.domainSeparator() on Rootstock testnet.
    expect(domainSeparatorOf(td)).toBe(
      "0xa31bca2c701187bad79a467434cb78c013c6c24055651d89ec0393733ea45905",
    );
  });

  for (const kind of ["deploy", "relay"] as const) {
    it(`hashes a ${kind} request the way the contracts do`, () => {
      const req = request(kind);
      const td = rifTypedData(req, 31);
      const domainSeparator = domainSeparatorOf(td);
      const expected = keccak256(
        `0x1901${domainSeparator.slice(2)}${contractStructHash(req).slice(2)}` as Hex,
      );
      expect(hashTypedData(viemTypedData(td))).toBe(expected);
    });
  }
});

const nativeRedeemAbi = parseAbiItem(
  "function redeem(bytes32 preimage, uint256 amount, address sender, uint256 timelock)",
);

const word = (hex: string) => `0x${hex.replace(/^0x/, "").padStart(64, "0")}`;

function ownerSigner(deployedNonce?: bigint): EvmSigner {
  return {
    address: owner.address,
    chainId: 31,
    signTypedData: (td) => owner.signTypedData(viemTypedData(td)),
    sendTransaction: () => Promise.reject(new Error("unused")),
    waitForReceipt: (hash) =>
      Promise.resolve({
        status: "success",
        blockNumber: 1n,
        transactionHash: hash,
        logs: [],
      }),
    getTransaction: () => Promise.reject(new Error("unused")),
    call: async ({ to, data }) => {
      if (data === "0xaffed0e0" && to.toLowerCase() === SMART_WALLET) {
        return deployedNonce === undefined
          ? "0x"
          : word(deployedNonce.toString(16));
      }
      if (data.startsWith("0x70ae92d2")) return word("4");
      throw new Error(`unexpected call ${to} ${data}`);
    },
  };
}

interface Posted {
  path: string;
  body: {
    relayRequest: {
      request: Record<string, unknown>;
      relayData: Record<string, string>;
    };
    metadata: { signature: string; relayMaxNonce: number };
  };
}

function relayServer(estimate: { gasPrice: string; estimation: string }) {
  const posted: Posted[] = [];
  const fetchFn = (async (url: string, init?: RequestInit) => {
    const path = url.replace(RELAY.server_url, "");
    const json = (body: unknown) =>
      new Response(JSON.stringify(body), { status: 200 });
    if (path === "/chain-info") {
      return json({
        feesReceiver: FEES_RECEIVER,
        relayHubAddress: RELAY.relay_hub,
        minGasPrice: "6352201",
        chainId: "31",
        ready: true,
      });
    }
    posted.push({ path, body: JSON.parse(String(init?.body)) });
    if (path === "/estimate") {
      return json({
        ...estimate,
        requiredNativeAmount: "0",
        requiredTokenAmount: "0",
        exchangeRate: "1",
      });
    }
    if (path === "/relay") {
      return json({ signedTx: "0x01", txHash: `0x${"ab".repeat(32)}` });
    }
    throw new Error(`unexpected ${path}`);
  }) as typeof fetch;
  return { posted, fetchFn };
}

describe("claimNativeViaRifRelay", () => {
  it("deploys the smart wallet and pays the fee out of the claim", async () => {
    const server = relayServer({ gasPrice: "7000000", estimation: "200000" });
    const result = await claimNativeViaRifRelay({
      swap,
      preimage: PREIMAGE,
      signer: ownerSigner(),
      relay: RELAY,
      fetch: server.fetchFn,
      now: () => 1_800_000_000,
    });

    expect(result.status).toBe("clientredeemed");
    expect(result.txHash).toBe(`0x${"ab".repeat(32)}`);

    const [estimate, relay] = server.posted;
    expect(estimate.path).toBe("/estimate");
    expect(estimate.body.metadata.signature).toBe("SERVER_SIGNATURE_REQUIRED");
    expect(estimate.body.relayRequest.request).toMatchObject({
      tokenAmount: "0",
      tokenGas: "0",
      index: 0,
      nonce: "4",
    });
    expect(estimate.body.relayRequest.request).not.toHaveProperty("gas");
    expect(estimate.body.relayRequest.relayData.gasPrice).toBe("6352201");

    // The server's higher price wins; fee = estimation × price + 10 %.
    const fee = rifFee(200_000n, 7_000_000n);
    expect(relay.path).toBe("/relay");
    expect(relay.body.relayRequest.request).toMatchObject({
      to: HTLC,
      tokenAmount: fee.toString(),
      tokenGas: "10500",
      validUntilTime: 1_800_000_000 + 2 * 24 * 60 * 60,
    });
    expect(relay.body.relayRequest.relayData).toMatchObject({
      gasPrice: "7000000",
      feesReceiver: FEES_RECEIVER,
      callForwarder: RELAY.smart_wallet_factory,
      callVerifier: RELAY.deploy_verifier,
    });

    // The owner signed exactly what was sent, over the factory's domain.
    const sent: RifRequest = {
      kind: "deploy",
      verifyingContract: RELAY.smart_wallet_factory,
      request: relay.body.relayRequest.request as Record<
        string,
        string | number
      >,
      relayData: relay.body.relayRequest.relayData as RifRequest["relayData"],
      relayHub: RELAY.relay_hub,
    };
    const recovered = await recoverTypedDataAddress({
      ...viemTypedData(rifTypedData(sent, 31)),
      signature: relay.body.metadata.signature as Hex,
    });
    expect(recovered).toBe(owner.address);

    // The smart wallet, the lock's claim address, redeems it directly.
    const { functionName, args } = decodeFunctionData({
      abi: [nativeRedeemAbi],
      data: relay.body.relayRequest.request.data as Hex,
    });
    expect(functionName).toBe("redeem");
    expect(args).toEqual([
      PREIMAGE,
      AMOUNT_WEI,
      "0xfD97fA3fCE801b67df009d09fA94F3f02147845e",
      1_800_000_000n,
    ]);
  });

  it("refuses a wallet that does not own the claim address", async () => {
    const server = relayServer({ gasPrice: "7000000", estimation: "200000" });
    const stranger = privateKeyToAccount(`0x${"22".repeat(32)}`);
    await expect(
      claimNativeViaRifRelay({
        swap,
        preimage: PREIMAGE,
        signer: { ...ownerSigner(), address: stranger.address },
        relay: RELAY,
        fetch: server.fetchFn,
      }),
    ).rejects.toThrow(/does not own this swap's claim address/);
    expect(server.posted).toEqual([]);
  });

  it("relays from a deployed wallet with the wallet's nonce and domain", async () => {
    const server = relayServer({ gasPrice: "6000000", estimation: "150000" });
    await claimNativeViaRifRelay({
      swap,
      preimage: PREIMAGE,
      signer: ownerSigner(9n),
      relay: RELAY,
      fetch: server.fetchFn,
    });

    const relay = server.posted[1];
    expect(relay.body.relayRequest.request).toMatchObject({
      nonce: "9",
      gas: "100000",
    });
    expect(relay.body.relayRequest.request).not.toHaveProperty("index");
    expect(relay.body.relayRequest.relayData).toMatchObject({
      // Below the floor: the request carries the floor price.
      gasPrice: "6352201",
      callForwarder: SMART_WALLET,
      callVerifier: RELAY.relay_verifier,
    });
    const sent: RifRequest = {
      kind: "relay",
      verifyingContract: SMART_WALLET,
      request: relay.body.relayRequest.request as Record<
        string,
        string | number
      >,
      relayData: relay.body.relayRequest.relayData as RifRequest["relayData"],
      relayHub: RELAY.relay_hub,
    };
    const recovered = await recoverTypedDataAddress({
      ...viemTypedData(rifTypedData(sent, 31)),
      signature: relay.body.metadata.signature as Hex,
    });
    expect(recovered).toBe(owner.address);
  });

  it("refuses a fee that would eat the claim", async () => {
    const server = relayServer({
      gasPrice: "1000000000",
      estimation: "200000",
    });
    await expect(
      claimNativeViaRifRelay({
        swap,
        preimage: PREIMAGE,
        signer: ownerSigner(),
        relay: RELAY,
        fetch: server.fetchFn,
      }),
    ).rejects.toThrow(/not below the claimed/);
    expect(server.posted.map((p) => p.path)).toEqual(["/estimate"]);
  });

  it("surfaces the server's HTTP-200 rejection", async () => {
    const fetchFn = (async (url: string) =>
      url.endsWith("/chain-info")
        ? new Response(
            JSON.stringify({
              feesReceiver: FEES_RECEIVER,
              relayHubAddress: RELAY.relay_hub,
              minGasPrice: "1",
              chainId: "31",
              ready: true,
            }),
          )
        : new Response(
            JSON.stringify({ error: "Token amount needs to be 0" }),
            {
              status: 200,
            },
          )) as typeof fetch;
    await expect(
      claimNativeViaRifRelay({
        swap,
        preimage: PREIMAGE,
        signer: ownerSigner(),
        relay: RELAY,
        fetch: fetchFn,
      }),
    ).rejects.toThrow(/Token amount needs to be 0/);
  });
});
