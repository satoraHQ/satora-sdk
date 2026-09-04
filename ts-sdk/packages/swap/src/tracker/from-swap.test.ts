import type { GetSwapResponse, StoredSwap } from "@lendasat/lendaswap-sdk-pure";
import { StrictVhtlcScript } from "@lendasat/lendaswap-sdk-pure";
import { ripemd160 } from "@noble/hashes/legacy.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { hex } from "@scure/base";
import { describe, expect, it } from "vitest";
import { swapToTracked } from "./from-swap.js";

const preimage = new Uint8Array(32).fill(7);
const hashLock = hex.encode(sha256(preimage));
// btc_to_arkade carries the HASH160 = ripemd160(sha256(preimage)) directly (20 bytes).
const hash160 = hex.encode(ripemd160(sha256(preimage)));

// Valid BIP340 x-only pubkeys (test vectors) — StrictVhtlcScript derives a taproot
// output, which requires on-curve keys.
const senderPk =
  "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";
const receiverPk =
  "dff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659";
const serverPk =
  "dd308afec5777e13121fa72b9cc1b7cc0139715309b086c960e18fd969774eb8";

// Arkade requires seconds-timelocks to be multiples of 512.
const DELAYS = {
  unilateral_claim_delay: 512,
  unilateral_refund_delay: 1024,
  unilateral_refund_without_receiver_delay: 1536,
};

/** The pkScript buildArkadeVhtlcRef should derive, computed independently here. */
const expectedArkadeScript = hex.encode(
  new StrictVhtlcScript({
    sender: hex.decode(senderPk),
    receiver: hex.decode(receiverPk),
    server: hex.decode(serverPk),
    preimageHash: ripemd160(hex.decode(hashLock)),
    refundLocktime: BigInt(1_000_000),
    unilateralClaimDelay: { type: "seconds", value: 512n },
    unilateralRefundDelay: { type: "seconds", value: 1024n },
    unilateralRefundWithoutReceiverDelay: { type: "seconds", value: 1536n },
  }).pkScript,
);

/**
 * The pkScript for btc_to_arkade's VHTLC: funder = server_vhtlc_pk (senderPk here),
 * claimer = the client's own key (receiverPk), and the HASH160 lock is used as the
 * preimageHash as-is (not hashed again).
 */
const expectedBtcArkadeScript = hex.encode(
  new StrictVhtlcScript({
    sender: hex.decode(senderPk),
    receiver: hex.decode(receiverPk),
    server: hex.decode(serverPk),
    preimageHash: hex.decode(hash160),
    refundLocktime: BigInt(1_000_000),
    unilateralClaimDelay: { type: "seconds", value: 512n },
    unilateralRefundDelay: { type: "seconds", value: 1024n },
    unilateralRefundWithoutReceiverDelay: { type: "seconds", value: 1536n },
  }).pkScript,
);

/** Build a StoredSwap wrapping a partial response; only the read fields matter. */
function stored(response: Partial<GetSwapResponse>): StoredSwap {
  return { response } as unknown as StoredSwap;
}

const arkadeEvmFields = {
  id: "swap-1",
  sender_pk: senderPk,
  receiver_pk: receiverPk,
  arkade_server_pk: serverPk,
  hash_lock: hashLock,
  btc_vhtlc_address: "ark1qexample",
  vhtlc_refund_locktime: 1_000_000,
  evm_refund_locktime: 900_000,
  evm_chain_id: 137,
  evm_htlc_address: "0xhtlc",
  evm_expected_sats: "1450",
  client_evm_address: "0xclient",
  server_evm_address: "0xserver",
  wbtc_address: "0xwbtc",
  source_amount: "1500",
  target_amount: "1400",
  ...DELAYS,
};

describe("swapToTracked", () => {
  it("maps arkade_to_evm: client funds Arkade, claims EVM", () => {
    const tracked = swapToTracked(
      stored({ ...arkadeEvmFields, direction: "arkade_to_evm" }),
    );
    expect(tracked).toBeDefined();
    expect(tracked?.swapId).toBe("swap-1");
    expect(tracked?.clientHtlc).toEqual({
      ledger: "arkade",
      script: expectedArkadeScript,
      address: "ark1qexample",
      preimageHash: hashLock,
      expectedSats: 1500, // source_amount
      params: expect.any(Object),
    });
    expect(tracked?.serverHtlc).toEqual({
      ledger: "evm",
      chainId: 137,
      htlc: "0xhtlc",
      preimageHash: `0x${hashLock}`,
      claimAddress: "0xclient", // the client claims the server's EVM HTLC
      expectedAmount: 1450n, // evm_expected_sats
      expectedToken: "0xwbtc",
      sender: "0xserver", // the server funded it
      timelockSec: 900_000,
    });
    // locktimes converted seconds → ms, client=Arkade, server=EVM
    expect(tracked?.clientRefundLocktime).toBe(1_000_000_000);
    expect(tracked?.serverRefundLocktime).toBe(900_000_000);
  });

  it("maps evm_to_arkade: legs and locktimes swap", () => {
    const tracked = swapToTracked(
      stored({ ...arkadeEvmFields, direction: "evm_to_arkade" }),
    );
    expect(tracked?.clientHtlc?.ledger).toBe("evm");
    expect(tracked?.serverHtlc?.ledger).toBe("arkade");
    expect(tracked?.clientRefundLocktime).toBe(900_000_000); // EVM leg
    expect(tracked?.serverRefundLocktime).toBe(1_000_000_000); // Arkade leg
  });

  it("preserves an already-0x-prefixed hash_lock for the EVM topic", () => {
    const tracked = swapToTracked(
      stored({
        ...arkadeEvmFields,
        hash_lock: `0x${hashLock}`,
        direction: "arkade_to_evm",
      }),
    );
    expect(tracked?.serverHtlc).toMatchObject({
      preimageHash: `0x${hashLock}`,
    });
  });

  const bitcoinEvmFields = {
    id: "swap-2",
    btc_htlc_address: "bcrt1qhtlc",
    evm_hash_lock: hashLock,
    btc_refund_locktime: 1_000_000,
    evm_refund_locktime: 900_000,
    evm_chain_id: 137,
    evm_htlc_address: "0xhtlc",
    evm_expected_sats: "2450",
    client_evm_address: "0xclient",
    server_evm_address: "0xserver",
    wbtc_address: "0xwbtc",
    source_amount: "2500",
    target_amount: "2400",
  };

  it("maps evm_to_bitcoin: client funds EVM, claims the BTC HTLC", () => {
    const tracked = swapToTracked(
      stored({ ...bitcoinEvmFields, direction: "evm_to_bitcoin" }),
    );
    expect(tracked?.clientHtlc).toEqual({
      ledger: "evm",
      chainId: 137,
      htlc: "0xhtlc",
      preimageHash: `0x${hashLock}`,
      claimAddress: "0xserver", // the server claims the client's EVM HTLC
      expectedAmount: 2450n, // evm_expected_sats
      expectedToken: "0xwbtc",
      sender: "0xclient", // the client funded it
      timelockSec: 900_000,
    });
    expect(tracked?.serverHtlc).toEqual({
      ledger: "bitcoin",
      address: "bcrt1qhtlc",
      preimageHash: hashLock, // sha256 hash, no 0x — the classifier verifies against it
      expectedSats: 2400, // target_amount (server funds the BTC leg)
      minConfirmations: undefined, // server-funded: the reader applies the configured depth
    });
    expect(tracked?.clientRefundLocktime).toBe(900_000_000); // EVM leg
    expect(tracked?.serverRefundLocktime).toBe(1_000_000_000); // BTC leg
  });

  it("maps bitcoin_to_evm: legs and locktimes swap", () => {
    const tracked = swapToTracked(
      stored({ ...bitcoinEvmFields, direction: "bitcoin_to_evm" }),
    );
    expect(tracked?.clientHtlc?.ledger).toBe("bitcoin");
    expect(tracked?.serverHtlc?.ledger).toBe("evm");
    expect(tracked?.clientRefundLocktime).toBe(1_000_000_000); // BTC leg
    expect(tracked?.serverRefundLocktime).toBe(900_000_000); // EVM leg
  });

  it("client-funded BTC leg waits a block, server-funded one follows the reader", () => {
    // The server does not act on the client's funding until it has a blocktime,
    // so reading it back at 0-conf would show the swap funded while the server
    // is still waiting. Its own funding is gated by the client's configured
    // depth, which the reader resolves on every read.
    const clientFunds = swapToTracked(
      stored({ ...bitcoinEvmFields, direction: "bitcoin_to_evm" }),
    );
    const serverFunds = swapToTracked(
      stored({ ...bitcoinEvmFields, direction: "evm_to_bitcoin" }),
    );

    const clientLeg = clientFunds?.clientHtlc;
    const serverLeg = serverFunds?.serverHtlc;
    if (clientLeg?.ledger !== "bitcoin" || serverLeg?.ledger !== "bitcoin")
      throw new Error("expected a Bitcoin leg on each side");

    expect(clientLeg.minConfirmations).toBe(1);
    expect(serverLeg.minConfirmations).toBeUndefined();
  });


  // ─── Lightning: one on-chain leg, the other side off-chain (undefined) ──────

  const lnArkadeFields = {
    ...arkadeEvmFields,
    id: "swap-ln-ark",
    arkade_vhtlc_address: "ark1qlnexample",
    boltz_amount_sats: 1450, // source minus fees — what actually gets locked
  };

  it("maps lightning_to_arkade: client claims Arkade, no client leg", () => {
    const tracked = swapToTracked(
      stored({ ...lnArkadeFields, direction: "lightning_to_arkade" }),
    );
    expect(tracked?.clientHtlc).toBeUndefined();
    expect(tracked?.serverHtlc).toEqual({
      ledger: "arkade",
      script: expectedArkadeScript,
      address: "ark1qlnexample",
      preimageHash: hashLock,
      expectedSats: 1400, // target_amount — what the client receives
      params: expect.any(Object),
    });
    expect(tracked?.clientRefundLocktime).toBe(0); // no on-chain client leg
    expect(tracked?.serverRefundLocktime).toBe(1_000_000_000); // vhtlc leg
  });

  it("maps lightning_to_evm: client claims the EVM HTLC, no client leg", () => {
    const tracked = swapToTracked(
      stored({
        ...bitcoinEvmFields,
        id: "swap-ln-evm",
        hash_lock: hashLock, // this route exposes hash_lock, not evm_hash_lock
        direction: "lightning_to_evm",
      }),
    );
    expect(tracked?.clientHtlc).toBeUndefined();
    expect(tracked?.serverHtlc).toEqual({
      ledger: "evm",
      chainId: 137,
      htlc: "0xhtlc",
      preimageHash: `0x${hashLock}`,
      claimAddress: "0xclient", // the client claims the server's EVM HTLC
      expectedAmount: 2450n,
      expectedToken: "0xwbtc",
      sender: "0xserver",
      timelockSec: 900_000,
      createdAtMs: undefined,
    });
    expect(tracked?.clientRefundLocktime).toBe(0); // no on-chain client leg
    expect(tracked?.serverRefundLocktime).toBe(900_000_000); // EVM leg
  });

  it("token-less directions fall back to the per-chain locked token on mainnet", () => {
    const tracked = swapToTracked(
      stored({
        ...arkadeEvmFields,
        direction: "evm_to_arkade",
        network: "bitcoin",
      } as Partial<GetSwapResponse>),
    );
    expect(tracked?.clientHtlc).toMatchObject({
      expectedToken: "0x1BFD67037B42Cf73acF2047067bd4F2C47D9BfD6", // WBTC (chain 137)
    });
  });

  it("no locked-token fallback off mainnet (a dev token lives elsewhere)", () => {
    const tracked = swapToTracked(
      stored({
        ...arkadeEvmFields,
        direction: "evm_to_arkade",
        network: "regtest",
      } as Partial<GetSwapResponse>),
    );
    expect(tracked?.clientHtlc).toMatchObject({ expectedToken: undefined });
  });

  it("maps btc_to_arkade: client funds BTC, claims the Arkade VHTLC (HASH160 lock)", () => {
    // The VHTLC's receiver key comes from storage (the client's own pubkey), not
    // the response; the funder is server_vhtlc_pk and the lock is a 20-byte HASH160.
    const tracked = swapToTracked({
      response: {
        direction: "btc_to_arkade",
        id: "swap-btc-ark",
        btc_htlc_address: "bcrt1qbtcark",
        hash_lock: hash160,
        server_vhtlc_pk: senderPk,
        arkade_server_pk: serverPk,
        arkade_vhtlc_address: "ark1qbtcark",
        vhtlc_refund_locktime: 1_000_000,
        btc_refund_locktime: 900_000,
        source_amount: "2500",
        target_amount: "2400",
        ...DELAYS,
      },
      publicKey: receiverPk, // the client's key — the VHTLC receiver
    } as unknown as StoredSwap);

    expect(tracked?.clientHtlc).toEqual({
      ledger: "bitcoin",
      address: "bcrt1qbtcark",
      preimageHash: hash160, // 20-byte HASH160, kept as-is
      expectedSats: 2500, // source_amount
      minConfirmations: 1, // the client funded it; the server waits for a block
    });
    expect(tracked?.serverHtlc).toEqual({
      ledger: "arkade",
      script: expectedBtcArkadeScript,
      address: "ark1qbtcark",
      preimageHash: hash160,
      expectedSats: 2400, // target_amount
      params: expect.any(Object),
    });
    expect(tracked?.clientRefundLocktime).toBe(900_000_000); // BTC leg
    expect(tracked?.serverRefundLocktime).toBe(1_000_000_000); // VHTLC leg
  });

  it("maps evm_to_lightning: client funds the EVM HTLC, no server leg", () => {
    const tracked = swapToTracked(
      stored({
        id: "swap-e2l",
        direction: "evm_to_lightning",
        hash_lock: hashLock,
        evm_chain_id: 137,
        evm_htlc_address: "0xhtlc",
        evm_coordinator_address: "0xcoordinator",
        evm_expected_sats: "1450",
        wbtc_address: "0xwbtc",
        client_evm_address: "0xclient",
        server_evm_address: "0xserver",
        evm_refund_locktime: 900_000,
        created_at: "2026-01-01T00:00:00Z",
      } as never),
    );

    expect(tracked?.serverHtlc).toBeUndefined();
    expect(tracked?.clientHtlc).toEqual({
      ledger: "evm",
      chainId: 137,
      htlc: "0xhtlc",
      preimageHash: `0x${hashLock}`,
      claimAddress: "0xserver", // the server claims the client's HTLC
      expectedAmount: 1450n,
      expectedToken: "0xwbtc",
      sender: "0xcoordinator",
      timelockSec: 900_000,
      createdAtMs: Date.parse("2026-01-01T00:00:00Z"),
    });
    expect(tracked?.clientRefundLocktime).toBe(900_000_000);
    expect(tracked?.serverRefundLocktime).toBe(0);
  });

  it("returns undefined for directions not yet mapped", () => {
    expect(
      swapToTracked(stored({ direction: "future_direction" } as never)),
    ).toBeUndefined();
  });
});
