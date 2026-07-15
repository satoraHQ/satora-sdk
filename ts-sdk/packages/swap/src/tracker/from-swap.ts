/**
 * Map a stored swap to the {@link TrackedSwap} the {@link SwapTracker} watches.
 *
 * The recovery bundle (`StoredSwap.response`) is a discriminated union over swap
 * directions; each carries the two HTLC legs and their refund locktimes. This
 * extracts the client-funded and server-funded HTLCs plus their locktimes so the
 * pure pipeline can derive the next action.
 *
 * Directions whose both legs are observable today (Arkade↔EVM, Bitcoin↔EVM) are
 * mapped; others return `undefined` until their ledger managers exist, rather than
 * producing a half-watchable swap.
 */
import type { StoredSwap } from "@lendasat/lendaswap-sdk-pure";
import { buildArkadeVhtlcRef } from "../contracts/arkade-vhtlc.js";
import type { HtlcRef } from "../contracts/types.js";
import type { TrackedSwap } from "./swap-tracker.js";

/** Locktimes are unix seconds on the wire; the resolver works in ms. */
const ms = (seconds: number): number => seconds * 1000;

const ensure0x = (value: string): `0x${string}` =>
  (value.startsWith("0x") ? value : `0x${value}`) as `0x${string}`;
const strip0x = (value: string): string =>
  value.startsWith("0x") ? value.slice(2) : value;

/**
 * The Arkade VHTLC leg. The three pubkeys the response reports are the exact keys
 * the server used to build the on-chain VHTLC, so feeding them back into
 * `VHTLC.Script` reproduces the funded pkScript regardless of which side funds or
 * claims — the manager watches by that script. `address` names the field the
 * response carries it in (`btc_vhtlc_address` for Arkade↔EVM, `arkade_vhtlc_address`
 * for Arkade↔Lightning).
 */
type ArkadeVhtlcFields = {
  sender_pk: string;
  receiver_pk: string;
  arkade_server_pk: string;
  hash_lock: string;
  vhtlc_refund_locktime: number;
  unilateral_claim_delay: number;
  unilateral_refund_delay: number;
  unilateral_refund_without_receiver_delay: number;
};

function arkadeLeg(
  r: ArkadeVhtlcFields,
  address: string,
  expectedSats: number,
): HtlcRef {
  return buildArkadeVhtlcRef({
    senderPk: r.sender_pk,
    receiverPk: r.receiver_pk,
    serverPk: r.arkade_server_pk,
    hashLock: r.hash_lock,
    address,
    refundLocktime: r.vhtlc_refund_locktime,
    unilateralClaimDelay: r.unilateral_claim_delay,
    unilateralRefundDelay: r.unilateral_refund_delay,
    unilateralRefundWithoutReceiverDelay:
      r.unilateral_refund_without_receiver_delay,
    expectedSats,
  });
}

/**
 * The EVM HTLC leg. `claimAddress` is who can claim (the client for a
 * server-funded leg, the server for the client's own); `expectedSats`/`token`
 * are what the HTLC must lock (the leg is `invalid` otherwise).
 */
function evmLeg(args: {
  chainId: number;
  htlc: string;
  hashLock: string;
  claimAddress: string;
  expectedSats: string;
  token?: string;
}): HtlcRef {
  return {
    ledger: "evm",
    chainId: args.chainId,
    htlc: ensure0x(args.htlc),
    preimageHash: ensure0x(args.hashLock),
    claimAddress: ensure0x(args.claimAddress),
    expectedAmount: BigInt(args.expectedSats),
    expectedToken: args.token ? ensure0x(args.token) : undefined,
  };
}

/**
 * The Bitcoin HTLC leg. `hashLock` is the SHA-256 preimage hash (`evm_hash_lock`);
 * the on-chain script commits `ripemd160` of it, but the classifier verifies a
 * revealed preimage as `sha256(preimage) === hashLock`.
 */
function bitcoinLeg(
  address: string,
  hashLock: string,
  expectedSats: number,
): HtlcRef {
  return {
    ledger: "bitcoin",
    address,
    preimageHash: strip0x(hashLock),
    expectedSats,
  };
}

export function swapToTracked(stored: StoredSwap): TrackedSwap | undefined {
  const r = stored.response;
  switch (r.direction) {
    // Client sends Arkade (funds the VHTLC with source_amount sats) and claims EVM.
    case "arkade_to_evm":
      return {
        swapId: r.id,
        clientHtlc: arkadeLeg(r, r.btc_vhtlc_address, Number(r.source_amount)),
        serverHtlc: evmLeg({
          chainId: r.evm_chain_id,
          htlc: r.evm_htlc_address,
          hashLock: r.hash_lock,
          claimAddress: r.client_evm_address, // the client claims the server's EVM HTLC
          expectedSats: r.evm_expected_sats,
          token: r.wbtc_address,
        }),
        clientRefundLocktime: ms(r.vhtlc_refund_locktime),
        serverRefundLocktime: ms(r.evm_refund_locktime),
      };
    // Client sends EVM and claims the Arkade VHTLC (server funds it with target_amount sats).
    case "evm_to_arkade":
      return {
        swapId: r.id,
        clientHtlc: evmLeg({
          chainId: r.evm_chain_id,
          htlc: r.evm_htlc_address,
          hashLock: r.hash_lock,
          claimAddress: r.server_evm_address, // the server claims the client's EVM HTLC
          expectedSats: r.evm_expected_sats,
          // evm_to_arkade doesn't expose the locked token — verify amount only.
        }),
        serverHtlc: arkadeLeg(r, r.btc_vhtlc_address, Number(r.target_amount)),
        clientRefundLocktime: ms(r.evm_refund_locktime),
        serverRefundLocktime: ms(r.vhtlc_refund_locktime),
      };
    // Client sends BTC (funds the on-chain HTLC with source_amount sats) and claims EVM.
    case "bitcoin_to_evm":
      return {
        swapId: r.id,
        clientHtlc: bitcoinLeg(
          r.btc_htlc_address,
          r.evm_hash_lock,
          Number(r.source_amount),
        ),
        serverHtlc: evmLeg({
          chainId: r.evm_chain_id,
          htlc: r.evm_htlc_address,
          hashLock: r.evm_hash_lock,
          claimAddress: r.client_evm_address, // the client claims the server's EVM HTLC
          expectedSats: r.evm_expected_sats,
          token: r.wbtc_address,
        }),
        clientRefundLocktime: ms(r.btc_refund_locktime),
        serverRefundLocktime: ms(r.evm_refund_locktime),
      };
    // Client sends EVM and claims the BTC HTLC (server funds it with target_amount sats).
    case "evm_to_bitcoin":
      return {
        swapId: r.id,
        clientHtlc: evmLeg({
          chainId: r.evm_chain_id,
          htlc: r.evm_htlc_address,
          hashLock: r.evm_hash_lock,
          claimAddress: r.server_evm_address, // the server claims the client's EVM HTLC
          expectedSats: r.evm_expected_sats,
          token: r.wbtc_address,
        }),
        serverHtlc: bitcoinLeg(
          r.btc_htlc_address,
          r.evm_hash_lock,
          Number(r.target_amount),
        ),
        clientRefundLocktime: ms(r.evm_refund_locktime),
        serverRefundLocktime: ms(r.btc_refund_locktime),
      };

    // Client sends BTC (funds the on-chain HTLC) and claims the Arkade VHTLC (the
    // server funds it). Unlike the other Arkade directions, the VHTLC's receiver is
    // the client's own key (not in the response) and its funder is `server_vhtlc_pk`;
    // the hash lock is a 20-byte HASH160, which buildArkadeVhtlcRef takes as-is.
    case "btc_to_arkade":
      return {
        swapId: r.id,
        clientHtlc: bitcoinLeg(
          r.btc_htlc_address,
          r.hash_lock,
          Number(r.source_amount),
        ),
        serverHtlc: buildArkadeVhtlcRef({
          senderPk: r.server_vhtlc_pk, // the server funds the VHTLC
          receiverPk: stored.publicKey, // the client claims it
          serverPk: r.arkade_server_pk,
          hashLock: r.hash_lock,
          address: r.arkade_vhtlc_address,
          refundLocktime: r.vhtlc_refund_locktime,
          unilateralClaimDelay: r.unilateral_claim_delay,
          unilateralRefundDelay: r.unilateral_refund_delay,
          unilateralRefundWithoutReceiverDelay:
            r.unilateral_refund_without_receiver_delay,
          expectedSats: Number(r.target_amount),
        }),
        clientRefundLocktime: ms(r.btc_refund_locktime),
        serverRefundLocktime: ms(r.vhtlc_refund_locktime),
      };

    // ─── Lightning: one on-chain leg, one off-chain LN payment ───────────────
    // Receive-on-Lightning: the client funds an on-chain HTLC and is paid over
    // Lightning. No server leg to watch — the server sweeping the client's leg
    // (with the preimage it got by paying the invoice) is the done signal.

    // Client funds the Arkade VHTLC; Boltz pays the invoice. The deposit is
    // `boltz_amount_sats` (source minus fees) — the amount actually locked, not
    // `source_amount` — else the client's own funding reads as under-funded.
    case "arkade_to_lightning":
      return {
        swapId: r.id,
        clientHtlc: arkadeLeg(
          r,
          r.arkade_vhtlc_address,
          Number(r.boltz_amount_sats),
        ),
        clientRefundLocktime: ms(r.vhtlc_refund_locktime),
        serverRefundLocktime: 0, // no on-chain server leg
      };
    // Client funds the EVM HTLC (server claims it after paying the invoice).
    case "evm_to_lightning":
      return {
        swapId: r.id,
        clientHtlc: evmLeg({
          chainId: r.evm_chain_id,
          htlc: r.evm_htlc_address,
          hashLock: r.hash_lock,
          claimAddress: r.server_evm_address, // the server claims the client's leg
          expectedSats: r.evm_expected_sats,
          // evm_to_lightning doesn't expose the locked token — verify amount only.
        }),
        clientRefundLocktime: ms(r.evm_refund_locktime),
        serverRefundLocktime: 0, // no on-chain server leg
      };

    // Pay-on-Lightning: the client pays a Lightning invoice (off-chain, nothing to
    // watch) and claims an on-chain HTLC. No client-funded leg — the client's
    // claim completes the swap; a hold invoice that never settles auto-unwinds.

    // Client pays the invoice, then claims the Arkade VHTLC (target_amount).
    case "lightning_to_arkade":
      return {
        swapId: r.id,
        serverHtlc: arkadeLeg(
          r,
          r.arkade_vhtlc_address,
          Number(r.target_amount),
        ),
        clientRefundLocktime: 0, // no on-chain client leg
        serverRefundLocktime: ms(r.vhtlc_refund_locktime),
      };
    // Client pays the invoice, then claims the EVM HTLC.
    case "lightning_to_evm":
      return {
        swapId: r.id,
        serverHtlc: evmLeg({
          chainId: r.evm_chain_id,
          htlc: r.evm_htlc_address,
          hashLock: r.hash_lock,
          claimAddress: r.client_evm_address, // the client claims the server's leg
          expectedSats: r.evm_expected_sats,
          token: r.wbtc_address,
        }),
        clientRefundLocktime: 0, // no on-chain client leg
        serverRefundLocktime: ms(r.evm_refund_locktime),
      };

    default:
      return undefined;
  }
}
