import { VHTLC } from "@arkade-os/sdk";
import type { GetSwapResponse, StoredSwap } from "@lendasat/lendaswap-sdk-pure";
import { ripemd160 } from "@noble/hashes/legacy.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { hex } from "@scure/base";
import { describe, expect, it } from "vitest";
import { swapToTracked } from "./from-swap.js";

const preimage = new Uint8Array(32).fill(7);
const hashLock = hex.encode(sha256(preimage));
// btc_to_arkade carries the HASH160 = ripemd160(sha256(preimage)) directly (20 bytes).
const hash160 = hex.encode(ripemd160(sha256(preimage)));

// Valid BIP340 x-only pubkeys (test vectors) — VHTLC.Script derives a taproot
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
  new VHTLC.Script({
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
  new VHTLC.Script({
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
    });
    expect(tracked?.serverHtlc).toEqual({
      ledger: "bitcoin",
      address: "bcrt1qhtlc",
      preimageHash: hashLock, // sha256 hash, no 0x — the classifier verifies against it
      expectedSats: 2400, // target_amount (server funds the BTC leg)
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

  // ─── Lightning: one on-chain leg, the other side off-chain (undefined) ──────

  const lnArkadeFields = {
    ...arkadeEvmFields,
    id: "swap-ln-ark",
    arkade_vhtlc_address: "ark1qlnexample",
    boltz_amount_sats: 1450, // source minus fees — what actually gets locked
  };

  it("maps arkade_to_lightning: client funds Arkade, no server leg", () => {
    const tracked = swapToTracked(
      stored({ ...lnArkadeFields, direction: "arkade_to_lightning" }),
    );
    expect(tracked?.serverHtlc).toBeUndefined();
    expect(tracked?.clientHtlc).toEqual({
      ledger: "arkade",
      script: expectedArkadeScript,
      address: "ark1qlnexample",
      preimageHash: hashLock,
      expectedSats: 1450, // boltz_amount_sats — what the client actually locks
      params: expect.any(Object),
    });
    expect(tracked?.clientRefundLocktime).toBe(1_000_000_000); // vhtlc leg
    expect(tracked?.serverRefundLocktime).toBe(0); // no on-chain server leg
  });

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

  it("maps evm_to_lightning: client funds EVM (server claims), no server leg", () => {
    const tracked = swapToTracked(
      stored({ ...arkadeEvmFields, direction: "evm_to_lightning" }),
    );
    expect(tracked?.serverHtlc).toBeUndefined();
    expect(tracked?.clientHtlc).toEqual({
      ledger: "evm",
      chainId: 137,
      htlc: "0xhtlc",
      preimageHash: `0x${hashLock}`,
      claimAddress: "0xserver", // the server claims the client's EVM HTLC
      expectedAmount: 1450n, // evm_expected_sats
      expectedToken: undefined, // no wbtc_address — amount-only
    });
    expect(tracked?.clientRefundLocktime).toBe(900_000_000); // EVM leg
    expect(tracked?.serverRefundLocktime).toBe(0);
  });

  it("maps lightning_to_evm: client claims EVM, no client leg", () => {
    const tracked = swapToTracked(
      stored({ ...arkadeEvmFields, direction: "lightning_to_evm" }),
    );
    expect(tracked?.clientHtlc).toBeUndefined();
    expect(tracked?.serverHtlc).toEqual({
      ledger: "evm",
      chainId: 137,
      htlc: "0xhtlc",
      preimageHash: `0x${hashLock}`,
      claimAddress: "0xclient", // the client claims the server's EVM HTLC
      expectedAmount: 1450n, // evm_expected_sats
      expectedToken: "0xwbtc",
    });
    expect(tracked?.clientRefundLocktime).toBe(0);
    expect(tracked?.serverRefundLocktime).toBe(900_000_000); // EVM leg
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

  it("returns undefined for directions not yet mapped", () => {
    expect(
      swapToTracked(stored({ direction: "future_direction" } as never)),
    ).toBeUndefined();
  });
});
