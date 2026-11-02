import {
  decodeFunctionData,
  type Hex,
  parseAbi,
  recoverTypedDataAddress,
} from "viem";
import { afterEach, describe, expect, it, vi } from "vitest";
import { deriveEvmAddress } from "../src/evm/signing.js";
import { Client, InMemorySwapStorage } from "../src/index.js";
import type { GaslessSwapResponse } from "../src/redeem/gasless.js";
import { claimViaSigner } from "../src/redeem/signer-claim.js";
import {
  HTLC,
  SERVER,
  SIGNER_ADDRESS,
  SWAP_ID,
  signerWith,
  storedSwap,
  TIMELOCK,
} from "./evm-funding-fixture.js";

const ZERO = "0x0000000000000000000000000000000000000000";
const key = new Uint8Array(32).fill(1);
const swap = {
  id: SWAP_ID,
  direction: "arkade_to_evm",
  evm_htlc_kind: "native",
  evm_htlc_address: HTLC,
  evm_coordinator_address: ZERO,
  evm_chain_id: 30,
  evm_htlc_version: 1,
  evm_expected_sats: "100000000000000",
  evm_refund_locktime: TIMELOCK,
  server_evm_address: SERVER,
  client_evm_address: deriveEvmAddress(key),
  target_evm_address: SIGNER_ADDRESS,
  wbtc_address: ZERO,
  target_token: { token_id: ZERO, chain: "30" },
} as unknown as GaslessSwapResponse & { direction: "arkade_to_evm" };
const params = {
  preimage: "00".repeat(32),
  secretKey: key,
  swap,
  destination: SIGNER_ADDRESS,
  calls: [],
  minAmountOut: BigInt(swap.evm_expected_sats),
};
const abi = parseAbi([
  "function redeemBySig(bytes32 preimage, uint256 amount, address sender, uint256 timelock, address destination, address sweepToken, uint256 minAmountOut, bytes32 callsHash, uint8 v, bytes32 r, bytes32 s) returns (address)",
]);

afterEach(() => vi.unstubAllGlobals());

describe("wallet-paid native claims", () => {
  it("calls HTLCNative directly in a legacy transaction with an authorization bound to the payout wallet", async () => {
    const sendTransaction = vi.fn(
      async (_tx: { to: string; data: string }) => "0xtx",
    );
    await claimViaSigner({
      ...params,
      signer: signerWith({ chainId: 30, sendTransaction }),
    });
    const tx = sendTransaction.mock.calls[0][0] as unknown as {
      to: string;
      data: Hex;
      type: string;
    };
    expect(tx.to).toBe(HTLC);
    expect(tx.type).toBe("legacy");
    const decoded = decodeFunctionData({ abi, data: tx.data });
    expect(decoded.functionName).toBe("redeemBySig");
    const [
      preimage,
      amount,
      sender,
      timelock,
      destination,
      sweepToken,
      minAmountOut,
      callsHash,
      v,
      r,
      s,
    ] = decoded.args;
    expect(destination.toLowerCase()).toBe(SIGNER_ADDRESS.toLowerCase());
    expect(
      await recoverTypedDataAddress({
        domain: {
          name: "HTLCNative",
          version: "1",
          chainId: 30,
          verifyingContract: HTLC,
        },
        types: {
          Redeem: [
            { name: "preimage", type: "bytes32" },
            { name: "amount", type: "uint256" },
            { name: "sender", type: "address" },
            { name: "timelock", type: "uint256" },
            { name: "caller", type: "address" },
            { name: "destination", type: "address" },
            { name: "sweepToken", type: "address" },
            { name: "minAmountOut", type: "uint256" },
            { name: "callsHash", type: "bytes32" },
          ],
        },
        primaryType: "Redeem",
        message: {
          preimage,
          amount,
          sender,
          timelock,
          caller: SIGNER_ADDRESS,
          destination,
          sweepToken,
          minAmountOut,
          callsHash,
        },
        signature: { r, s, v: BigInt(v) },
      }),
    ).toBe(deriveEvmAddress(key));
  });

  it("refuses to redirect the payout to a different submitting wallet", async () => {
    const sendTransaction = vi.fn(
      async (_tx: { to: string; data: string }) => "0xtx",
    );
    await expect(
      claimViaSigner({
        ...params,
        signer: signerWith({ chainId: 30, address: SERVER, sendTransaction }),
      }),
    ).rejects.toThrow(/payout wallet/);
    expect(sendTransaction).not.toHaveBeenCalled();
  });

  it("rejects a successful cancellation receipt and allows the claim to be retried", async () => {
    const sendTransaction = vi.fn(async () => "0xsubmitted");
    const waitForReceipt = vi
      .fn()
      .mockResolvedValueOnce({
        status: "success",
        blockNumber: 1n,
        transactionHash: "0xcancelled",
        logs: [],
      })
      .mockResolvedValueOnce({
        status: "success",
        blockNumber: 2n,
        transactionHash: "0xsubmitted",
        logs: [],
      });
    const signer = signerWith({
      chainId: 30,
      sendTransaction,
      waitForReceipt,
      getTransaction: async () => ({
        to: SIGNER_ADDRESS,
        from: SIGNER_ADDRESS,
        input: "0x",
      }),
    });
    await expect(claimViaSigner({ ...params, signer })).rejects.toThrow(
      /cancelled/,
    );
    await expect(claimViaSigner({ ...params, signer })).resolves.toMatchObject({
      status: "clientredeemed",
      txHash: "0xsubmitted",
    });
    expect(sendTransaction).toHaveBeenCalledTimes(2);
  });

  it("returns the mined replacement hash when the wallet speeds up the same claim", async () => {
    const sendTransaction = vi.fn(
      async (_tx: { to: string; data: string }) => "0xsubmitted",
    );
    const getTransaction = vi.fn(async () => ({
      to: HTLC,
      from: SIGNER_ADDRESS,
      input: sendTransaction.mock.calls[0][0].data,
    }));
    const result = await claimViaSigner({
      ...params,
      signer: signerWith({
        chainId: 30,
        sendTransaction,
        getTransaction,
        waitForReceipt: async () => ({
          status: "success",
          blockNumber: 1n,
          transactionHash: "0xrepriced",
        }),
      }),
    });
    expect(result.txHash).toBe("0xrepriced");
    expect(getTransaction).toHaveBeenCalledWith("0xrepriced");
  });

  it.each([
    "null",
    "undefined",
    "error",
  ])("reports an unverified replacement rather than cancellation when the RPC returns %s", async (response) => {
    const getTransaction = vi.fn();
    if (response === "error") {
      getTransaction.mockRejectedValue(new Error("RPC unavailable"));
    } else {
      getTransaction.mockResolvedValue(response === "null" ? null : undefined);
    }
    const sendTransaction = vi.fn(async () => "0xsubmitted");
    const result = claimViaSigner({
      ...params,
      signer: signerWith({
        chainId: 30,
        sendTransaction,
        getTransaction,
        waitForReceipt: async () => ({
          status: "success",
          blockNumber: 1n,
          transactionHash: "0xreplacement",
        }),
      }),
    });
    await expect(result).rejects.toThrow(
      /0xreplacement was mined, but its claim could not yet be verified/,
    );
    await expect(result).rejects.not.toThrow(/cancelled|try again/);
    expect(sendTransaction).toHaveBeenCalledOnce();
  });

  it.each([
    { label: "different HTLC", to: SERVER },
    { label: "different claim calldata", input: "0x1234" },
    { label: "different sender", from: SERVER },
  ])("rejects a successful replacement with $label", async ({
    label: _label,
    ...replacement
  }) => {
    const sendTransaction = vi.fn(
      async (_tx: { to: string; data: string }) => "0xsubmitted",
    );
    await expect(
      claimViaSigner({
        ...params,
        signer: signerWith({
          chainId: 30,
          sendTransaction,
          getTransaction: async () => ({
            to: HTLC,
            from: SIGNER_ADDRESS,
            input: sendTransaction.mock.calls[0][0].data,
            ...replacement,
          }),
          waitForReceipt: async () => ({
            status: "success",
            blockNumber: 1n,
            transactionHash: "0xreplacement",
          }),
        }),
      }),
    ).rejects.toThrow(/replaced or cancelled/);
  });

  it("Client.claim requires a paying wallet and never falls back to backend sponsorship", async () => {
    const storage = new InMemorySwapStorage();
    await storage.store(
      storedSwap({ response: swap, secretKey: "01".repeat(32) }),
    );
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : String(input);
      if (url.includes("chain-config")) return Response.json({ chains: [] });
      if (url.includes("claim-gasless"))
        throw new Error("backend relay must never be called");
      return Response.json(swap);
    });
    vi.stubGlobal("fetch", fetchMock);
    const client = await Client.builder()
      .withBaseUrl("https://example.test")
      .withSwapStorage(storage)
      .build();
    expect(client.canSponsorClaim(swap)).toBe(false);
    await expect(client.claim(SWAP_ID)).rejects.toThrow(
      /Rootstock RIF sponsorship is not configured/,
    );
    const sendTransaction = vi.fn(
      async (_tx: { to: string; data: string }) => "0xtx",
    );
    const result = await client.claim(SWAP_ID, {
      signer: signerWith({ chainId: 30, sendTransaction }),
    });
    expect(result.success).toBe(true);
    expect(sendTransaction).toHaveBeenCalledOnce();
    expect(
      fetchMock.mock.calls.every(
        ([input]) =>
          !(input instanceof Request ? input.url : String(input)).includes(
            "claim-gasless",
          ),
      ),
    ).toBe(true);
  });
});
