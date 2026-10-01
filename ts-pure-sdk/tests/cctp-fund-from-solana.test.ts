import { ed25519 } from "@noble/curves/ed25519.js";
import { base58 } from "@scure/base";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Everything that touches a chain is mocked: the Solana RPC, IRIS, and the
// Arbitrum smart account + UserOp. The tests exercise the real instruction
// and transaction encoding, `cctpFundSwapFromSolana`, and the `Client` wiring
// (owner key, burn recording, resume, recovery).
const rpc = {
  getLatestBlockhash: vi.fn(),
  getBlockHeight: vi.fn(),
  getSignatureStatus: vi.fn(),
  getTokenAccountAmount: vi.fn(),
  simulateTransaction: vi.fn(),
  suggestPriorityFee: vi.fn(),
};
const fetchAttestation = vi.fn();
const hasCctpMessage = vi.fn();
const createSwapSmartAccountClient = vi.fn();
const submitCctpInboundUserOp = vi.fn();
const recoverCctpInbound = vi.fn();

vi.mock("../src/cctp-inbound/solana/rpc.js", () => ({
  getLatestBlockhash: (...a: unknown[]) => rpc.getLatestBlockhash(...a),
  getBlockHeight: (...a: unknown[]) => rpc.getBlockHeight(...a),
  getSignatureStatus: (...a: unknown[]) => rpc.getSignatureStatus(...a),
  getTokenAccountAmount: (...a: unknown[]) => rpc.getTokenAccountAmount(...a),
  simulateTransaction: (...a: unknown[]) => rpc.simulateTransaction(...a),
  suggestPriorityFee: (...a: unknown[]) => rpc.suggestPriorityFee(...a),
}));
vi.mock("../src/cctp/attestation.js", () => ({
  fetchAttestation: (...args: unknown[]) => fetchAttestation(...args),
  hasCctpMessage: (...args: unknown[]) => hasCctpMessage(...args),
}));
vi.mock("../src/cctp-inbound/smartAccount.js", () => ({
  createSwapSmartAccountClient: (...args: unknown[]) =>
    createSwapSmartAccountClient(...args),
}));
vi.mock("../src/cctp-inbound/submit.js", () => ({
  submitCctpInboundUserOp: (...args: unknown[]) =>
    submitCctpInboundUserOp(...args),
  submitCctpInboundBalanceUserOp: vi.fn(),
}));
vi.mock("../src/cctp-inbound/recover.js", () => ({
  recoverCctpInbound: (...args: unknown[]) => recoverCctpInbound(...args),
}));

import {
  buildDepositForBurnInstruction,
  buildSolanaBurnTransaction,
  DEPOSIT_FOR_BURN_DISCRIMINATOR,
  deriveAssociatedTokenAccount,
  SOLANA_BURN_COMPUTE_UNIT_LIMIT,
} from "../src/cctp-inbound/solana/burn.js";
import {
  SOLANA_MESSAGE_TRANSMITTER_V2,
  SOLANA_TOKEN_MESSENGER_MINTER_V2,
  SOLANA_USDC_MINT,
} from "../src/cctp-inbound/solana/constants.js";
import { cctpFundSwapFromSolana } from "../src/cctp-inbound/solana/fundSwap.js";
import type { SolanaSigner } from "../src/cctp-inbound/solana/types.js";
import type { EvmSigner } from "../src/evm/wallet.js";
import { Client, InMemorySwapStorage } from "../src/index.js";
import { SWAP_ID, storedSwap, swapResponse } from "./evm-funding-fixture.js";

/** A wallet pubkey that exists on mainnet (a CCTP burner). */
const OWNER = "7QgQPtsZqs7dcckXyyWxsQJ9GHaLRwFGM7mGWjHg6DcM";
const KERNEL = "0x0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a";
const SIGNATURE = base58.encode(new Uint8Array(64).fill(9));
const BLOCKHASH = base58.encode(new Uint8Array(32).fill(7));
const USER_OP = `0x${"0e".repeat(32)}` as const;
const SETTLE_TX = `0x${"5e".repeat(32)}` as const;
const AA = { bundlerUrl: "http://bundler.test" };
const RPC_URL = "http://solana.test";

/**
 * Mainnet PDAs of the CCTPv2 programs, checked against the live accounts
 * (owner = the programs; the remote token messenger's data is domain 3 +
 * the EVM TokenMessengerV2).
 */
const MESSAGE_TRANSMITTER = "W1k5ijkaSTo5iA5zChNpfzcy796fLhkBxfmJuR8W8HU";
const TOKEN_MESSENGER = "AawthJCGRmggpfv9MMWV6Jmo9cue4gL9wUZgRBShg58W";
const REMOTE_TOKEN_MESSENGER_ARBITRUM =
  "53NSDvEXmUixWSsCQF5rnTxcKdihq19WXNaZFXrX3ojf";
const TOKEN_MINTER = "E1bQJ8eMMn3zmeSewW3HQ8zmJr7KR75JonbwAtWx2bux";
const LOCAL_TOKEN_USDC = "CRBBbuLCyrkQy4dCTHxqstSmDQv4ajBeUVb9qUdMVaP1";

function wallet(): SolanaSigner & { sent: Uint8Array[] } {
  const sent: Uint8Array[] = [];
  return {
    publicKey: OWNER,
    sent,
    signAndSendTransaction: async (tx) => {
      sent.push(tx);
      return SIGNATURE;
    },
  };
}

function evmOwner(address = KERNEL): EvmSigner {
  return { address, chainId: 42161 } as EvmSigner;
}

beforeEach(() => {
  rpc.getLatestBlockhash.mockResolvedValue({
    blockhash: BLOCKHASH,
    lastValidBlockHeight: 100,
  });
  rpc.getBlockHeight.mockResolvedValue(50);
  rpc.getSignatureStatus.mockResolvedValue({
    confirmationStatus: "confirmed",
    err: null,
  });
  rpc.getTokenAccountAmount.mockResolvedValue(10_000_000n);
  rpc.simulateTransaction.mockResolvedValue(null);
  rpc.suggestPriorityFee.mockResolvedValue(10_000n);
  fetchAttestation.mockResolvedValue({ message: "0x01", attestation: "0x02" });
  hasCctpMessage.mockResolvedValue(false);
  createSwapSmartAccountClient.mockImplementation(
    async ({ signer }: { signer: EvmSigner }) => ({
      accountAddress: signer.address,
    }),
  );
  submitCctpInboundUserOp.mockResolvedValue({
    userOpHash: USER_OP,
    transactionHash: SETTLE_TX,
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

describe("Solana deposit_for_burn instruction", () => {
  const params = {
    owner: OWNER,
    messageSentEventData: "11111111111111111111111111111112",
    amount: 1_234_567n,
    destinationDomain: 3,
    mintRecipient: KERNEL,
    destinationCaller: KERNEL,
    maxFee: 99n,
    minFinalityThreshold: 1000,
  };

  it("targets TokenMessengerMinterV2 with the IDL's account order", () => {
    const ix = buildDepositForBurnInstruction(params);
    expect(ix.programId).toBe(SOLANA_TOKEN_MESSENGER_MINTER_V2);
    expect(ix.keys.map((k) => [k.pubkey, k.isSigner, k.isWritable])).toEqual([
      [OWNER, true, false], // owner
      [OWNER, true, true], // event_rent_payer
      [expect.any(String), false, false], // sender_authority_pda
      [deriveAssociatedTokenAccount(OWNER), false, true], // burn_token_account
      [expect.any(String), false, false], // denylist_account
      [MESSAGE_TRANSMITTER, false, true],
      [TOKEN_MESSENGER, false, false],
      [REMOTE_TOKEN_MESSENGER_ARBITRUM, false, false],
      [TOKEN_MINTER, false, false],
      [LOCAL_TOKEN_USDC, false, true],
      [SOLANA_USDC_MINT, false, true], // burn_token_mint
      [params.messageSentEventData, true, true],
      [SOLANA_MESSAGE_TRANSMITTER_V2, false, false],
      [SOLANA_TOKEN_MESSENGER_MINTER_V2, false, false],
      ["TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", false, false],
      ["11111111111111111111111111111111", false, false],
      [expect.any(String), false, false], // event_authority
      [SOLANA_TOKEN_MESSENGER_MINTER_V2, false, false], // program
    ]);
  });

  it("encodes the params behind the Anchor discriminator", () => {
    const { data } = buildDepositForBurnInstruction(params);
    const view = new DataView(data.buffer, data.byteOffset);
    const kernel32 = new Uint8Array(32);
    kernel32.set(new Uint8Array(20).fill(0x0a), 12);

    expect(data.length).toBe(96);
    expect(data.subarray(0, 8)).toEqual(DEPOSIT_FOR_BURN_DISCRIMINATOR);
    expect(view.getBigUint64(8, true)).toBe(1_234_567n);
    expect(view.getUint32(16, true)).toBe(3);
    expect(data.subarray(20, 52)).toEqual(kernel32); // mint_recipient
    expect(data.subarray(52, 84)).toEqual(kernel32); // destination_caller
    expect(view.getBigUint64(84, true)).toBe(99n);
    expect(view.getUint32(92, true)).toBe(1000);
  });

  it("leaves the fee payer's signature to the wallet", () => {
    const secret = ed25519.utils.randomSecretKey();
    const burn = buildSolanaBurnTransaction({
      ...params,
      recentBlockhash: BLOCKHASH,
      priorityFeeMicroLamports: 5n,
      eventDataSecretKey: secret,
    });
    const tx = burn.transaction;

    // Two signatures: owner (fee payer, empty) then the event-data account.
    expect(tx[0]).toBe(2);
    expect(tx.subarray(1, 65)).toEqual(new Uint8Array(64));
    const eventSig = tx.subarray(65, 129);
    expect(tx.subarray(129)).toEqual(burn.message);
    expect(
      ed25519.verify(
        eventSig,
        burn.message,
        base58.decode(burn.messageSentEventData),
      ),
    ).toBe(true);
    // Header: 2 signers, both writable; account keys start with the owner.
    expect([...burn.message.subarray(0, 3)]).toEqual([2, 0, 11]);
    expect(base58.encode(burn.message.subarray(4, 36))).toBe(OWNER);
    expect(base58.encode(burn.message.subarray(36, 68))).toBe(
      burn.messageSentEventData,
    );
    expect(SOLANA_BURN_COMPUTE_UNIT_LIMIT).toBeGreaterThan(42_000);
  });
});

describe("cctpFundSwapFromSolana", () => {
  const context = { apiClient: {} as never, aa: AA };
  const base = {
    swapId: SWAP_ID,
    ownerSigner: evmOwner(),
    rpcUrl: RPC_URL,
    amount: 1_000_000n,
    maxFee: 100n,
    pollIntervalMs: 0,
  };

  it("burns to the owner's Kernel account and settles", async () => {
    const signer = wallet();
    const onBurn = vi.fn();
    const result = await cctpFundSwapFromSolana(context, {
      ...base,
      signer,
      onBurn,
    });

    expect(signer.sent).toHaveLength(1);
    // The Kernel address is pinned as mint recipient in the sent burn.
    const hex = Buffer.from(signer.sent[0]).toString("hex");
    expect(hex).toContain(`${"00".repeat(12)}${KERNEL.slice(2)}`.repeat(2));
    expect(onBurn.mock.calls).toEqual([[SIGNATURE, 100]]);
    expect(fetchAttestation).toHaveBeenCalledWith(
      expect.objectContaining({ sourceChain: "Solana", txHash: SIGNATURE }),
    );
    expect(submitCctpInboundUserOp).toHaveBeenCalledWith(
      context,
      expect.objectContaining({ signer: base.ownerSigner }),
    );
    expect(result).toMatchObject({
      burnSignature: SIGNATURE,
      transactionHash: SETTLE_TX,
      smartAccountAddress: KERNEL,
    });
  });

  it("refuses to burn more USDC than the wallet holds", async () => {
    rpc.getTokenAccountAmount.mockResolvedValue(5n);
    const signer = wallet();
    await expect(
      cctpFundSwapFromSolana(context, { ...base, signer }),
    ).rejects.toThrow(/Not enough USDC/);
    expect(signer.sent).toHaveLength(0);
  });

  it("does not prompt the wallet when the simulation fails", async () => {
    rpc.simulateTransaction.mockResolvedValue({
      err: { InstructionError: [2, { Custom: 1 }] },
      logs: ["Program log: boom"],
    });
    const signer = wallet();
    await expect(
      cctpFundSwapFromSolana(context, { ...base, signer }),
    ).rejects.toThrow(/would fail/);
    expect(signer.sent).toHaveLength(0);
  });

  it("clears the burn when it failed on-chain", async () => {
    rpc.getSignatureStatus.mockResolvedValue({
      confirmationStatus: "confirmed",
      err: { InstructionError: [2, { Custom: 1 }] },
    });
    const onBurn = vi.fn();
    await expect(
      cctpFundSwapFromSolana(context, { ...base, signer: wallet(), onBurn }),
    ).rejects.toThrow(/failed on Solana/);
    expect(onBurn.mock.calls).toEqual([[SIGNATURE, 100], [undefined]]);
    expect(fetchAttestation).not.toHaveBeenCalled();
  });

  it("clears the burn when its blockhash expired unseen", async () => {
    rpc.getSignatureStatus.mockResolvedValue(null);
    rpc.getBlockHeight.mockResolvedValue(101);
    const onBurn = vi.fn();
    await expect(
      cctpFundSwapFromSolana(context, { ...base, signer: wallet(), onBurn }),
    ).rejects.toThrow(/never landed/);
    expect(onBurn.mock.calls).toEqual([[SIGNATURE, 100], [undefined]]);
    expect(hasCctpMessage).toHaveBeenCalledWith(
      expect.objectContaining({ sourceChain: "Solana", txHash: SIGNATURE }),
    );
  });

  it("keeps polling an unseen burn until its blockhash expires", async () => {
    rpc.getSignatureStatus
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(null)
      .mockResolvedValue({ confirmationStatus: "confirmed", err: null });
    rpc.getBlockHeight.mockResolvedValue(100);
    const onBurn = vi.fn();
    await cctpFundSwapFromSolana(context, {
      ...base,
      signer: wallet(),
      onBurn,
    });
    expect(onBurn.mock.calls).toEqual([[SIGNATURE, 100]]);
    expect(hasCctpMessage).not.toHaveBeenCalled();
  });

  describe("resuming a burn the RPC cannot find", () => {
    const resume = (extra: { burnLastValidBlockHeight?: number } = {}) => {
      const signer = wallet();
      const onBurn = vi.fn();
      const run = cctpFundSwapFromSolana(context, {
        ...base,
        signer,
        onBurn,
        burnSignature: SIGNATURE,
        ...extra,
      });
      return { signer, onBurn, run };
    };

    beforeEach(() => {
      rpc.getSignatureStatus.mockResolvedValue(null);
      rpc.getBlockHeight.mockResolvedValue(101);
    });

    it("settles a burn IRIS knows, though the RPC lacks its history", async () => {
      hasCctpMessage.mockResolvedValue(true);
      const { signer, onBurn, run } = resume({ burnLastValidBlockHeight: 100 });

      await run;

      expect(signer.sent).toHaveLength(0);
      expect(onBurn).not.toHaveBeenCalled();
      expect(submitCctpInboundUserOp).toHaveBeenCalled();
    });

    it("clears it once past its expiry height and unknown to IRIS", async () => {
      const { onBurn, run } = resume({ burnLastValidBlockHeight: 100 });

      await expect(run).rejects.toThrow(/never landed/);
      expect(onBurn.mock.calls).toEqual([[undefined]]);
    });

    it("keeps it when IRIS cannot be asked", async () => {
      hasCctpMessage.mockRejectedValue(new Error("IRIS API error: 503"));
      const { onBurn, run } = resume({ burnLastValidBlockHeight: 100 });

      await expect(run).rejects.toThrow(/IRIS API error/);
      expect(onBurn).not.toHaveBeenCalled();
    });

    it("keeps it, however long it stays unseen, without an expiry height", async () => {
      // Long past the lookup window, yet only the chain can show the burn
      // expired, and its expiry height was not recorded.
      const start = Date.now();
      let calls = 0;
      vi.spyOn(Date, "now").mockImplementation(
        () => start + (calls++ > 0 ? 3_600_000 : 0),
      );
      const { onBurn, run } = resume();

      await expect(run).rejects.toThrow(/not visible on Solana yet/);
      expect(onBurn).not.toHaveBeenCalled();
      expect(hasCctpMessage).not.toHaveBeenCalled();
    });
  });

  it("resumes from a known burn without burning again", async () => {
    const signer = wallet();
    const onBurn = vi.fn();
    await cctpFundSwapFromSolana(context, {
      ...base,
      signer,
      onBurn,
      burnSignature: SIGNATURE,
    });

    expect(signer.sent).toHaveLength(0);
    expect(onBurn).not.toHaveBeenCalled();
    expect(fetchAttestation).toHaveBeenCalledWith(
      expect.objectContaining({ sourceChain: "Solana", txHash: SIGNATURE }),
    );
  });
});

describe("Client.fundSwapFromSolana", () => {
  function solanaSwap(depositor: string, status = "pending") {
    return {
      ...swapResponse({ clientEvmAddress: depositor, gasless: true }),
      status,
      source_amount: "1000000",
      bridge_source_chain: "Solana",
      bridge_source_token_address: SOLANA_USDC_MINT,
    } as ReturnType<typeof swapResponse>;
  }

  async function clientWith(
    depositorOf: (sdkAddress: string) => string,
    extra: {
      cctpBurnTxid?: string;
      cctpBurnLastValidBlockHeight?: number;
      status?: string;
      bridge?: string;
    } = {},
  ) {
    let response: ReturnType<typeof solanaSwap> | undefined;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = input instanceof Request ? input.url : String(input);
        if (url.endsWith(`/swap/${SWAP_ID}`) && response) {
          return Response.json(response, { status: 200 });
        }
        throw new Error(`unexpected request ${url}`);
      }),
    );
    const storage = new InMemorySwapStorage();
    const client = await Client.builder()
      .withSwapStorage(storage)
      .withAa(AA)
      .withSolanaRpcUrl(RPC_URL)
      .build();
    response = solanaSwap(depositorOf(client.getEvmAddress()), extra.status);
    if (extra.bridge) {
      (response as { bridge_source_chain?: string }).bridge_source_chain =
        extra.bridge;
    }
    await storage.store(
      storedSwap({
        response,
        cctpBurnTxid: extra.cctpBurnTxid,
        cctpBurnLastValidBlockHeight: extra.cctpBurnLastValidBlockHeight,
      }),
    );
    return { client, storage };
  }

  it("settles with the SDK's own key and records the burn", async () => {
    const { client, storage } = await clientWith((sdk) => sdk);

    const { txHash } = await client.fundSwapFromSolana(SWAP_ID, wallet(), {
      maxFee: 100n,
    });

    expect(txHash).toBe(SETTLE_TX);
    const owner = submitCctpInboundUserOp.mock.calls[0][1].signer as EvmSigner;
    expect(owner.address.toLowerCase()).toBe(
      client.getEvmAddress().toLowerCase(),
    );
    expect(owner.signAuthorization).toBeTypeOf("function");
    expect(await storage.get(SWAP_ID)).toMatchObject({
      cctpBurnTxid: SIGNATURE,
      cctpBurnLastValidBlockHeight: 100,
    });
    expect(rpc.getLatestBlockhash).toHaveBeenCalledWith(RPC_URL);
  });

  it("refuses a swap whose depositor is not the SDK key", async () => {
    const { client } = await clientWith(() => `0x${"99".repeat(20)}`);

    await expect(
      client.fundSwapFromSolana(SWAP_ID, wallet(), { maxFee: 100n }),
    ).rejects.toThrow(/gasless: true/);
  });

  it("resumes from the recorded burn instead of burning again", async () => {
    const { client } = await clientWith((sdk) => sdk, {
      cctpBurnTxid: SIGNATURE,
    });
    const signer = wallet();

    await client.fundSwapFromSolana(SWAP_ID, signer, { maxFee: 100n });

    expect(signer.sent).toHaveLength(0);
    expect(fetchAttestation).toHaveBeenCalledWith(
      expect.objectContaining({ sourceChain: "Solana", txHash: SIGNATURE }),
    );
  });

  it("resumes against the recorded expiry height", async () => {
    rpc.getSignatureStatus.mockResolvedValue(null);
    rpc.getBlockHeight.mockResolvedValue(101);
    const { client, storage } = await clientWith((sdk) => sdk, {
      cctpBurnTxid: SIGNATURE,
      cctpBurnLastValidBlockHeight: 100,
    });

    await expect(
      client.fundSwapFromSolana(SWAP_ID, wallet(), { maxFee: 100n }),
    ).rejects.toThrow(/never landed/);
    expect(await storage.get(SWAP_ID)).toMatchObject({
      cctpBurnTxid: undefined,
      cctpBurnLastValidBlockHeight: undefined,
    });
  });

  it("does not resume a swap the recorded burn already funded", async () => {
    const { client } = await clientWith((sdk) => sdk, {
      cctpBurnTxid: SIGNATURE,
      status: "clientfundingseen",
    });

    await expect(
      client.fundSwapFromSolana(SWAP_ID, wallet(), { maxFee: 100n }),
    ).rejects.toThrow(/already clientfundingseen/);
    expect(submitCctpInboundUserOp).not.toHaveBeenCalled();
  });

  it("rejects a swap not funded from Solana", async () => {
    const { client } = await clientWith((sdk) => sdk, { bridge: "Optimism" });

    await expect(
      client.fundSwapFromSolana(SWAP_ID, wallet(), { maxFee: 100n }),
    ).rejects.toThrow(/not funded from Solana/);
  });

  it("points EVM-signer funding of a Solana swap at fundSwapFromSolana", async () => {
    const { client } = await clientWith((sdk) => sdk);

    await expect(
      client.fundSwap(SWAP_ID, evmOwner(), { maxFee: 100n }),
    ).rejects.toThrow(/fundSwapFromSolana/);
  });

  it("recovers from the recorded Solana burn", async () => {
    const { client } = await clientWith((sdk) => sdk, {
      cctpBurnTxid: SIGNATURE,
    });
    recoverCctpInbound.mockResolvedValue({ sweepUserOpHash: USER_OP });
    const recipient = `0x${"77".repeat(20)}`;

    await client.recoverCctpInboundSwap(SWAP_ID, recipient);

    const [, params] = recoverCctpInbound.mock.calls[0];
    expect(params).toMatchObject({
      burnTxHash: SIGNATURE,
      sourceChain: "Solana",
      recipient,
    });
  });

  it("creates a Solana-sourced swap only gasless", async () => {
    const { client } = await clientWith((sdk) => sdk);

    await expect(
      client.createSwap({
        source: { chain: "Solana", tokenId: SOLANA_USDC_MINT },
        target: { chain: "Bitcoin", tokenId: "btc" },
        sourceAmount: 1_000_000,
        targetAddress: "bc1qexample",
      } as never),
    ).rejects.toThrow(/gasless: true/);
  });
});
