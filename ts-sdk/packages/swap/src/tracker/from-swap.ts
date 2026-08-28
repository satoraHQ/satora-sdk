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

/**
 * The BTC-pegged token the coordinator locks in HTLCs per MAINNET chain
 * (mirrors `config.mainnet.yaml` `tokens.wbtc`): WBTC on Polygon, tBTC on
 * Ethereum/Arbitrum. TEMPORARY fallback for the responses that don't expose
 * the locked token (`evm_to_arkade`) so their isActive
 * tuple is complete; delete once the server returns the token there.
 * Gated to mainnet: a dev deployment's mock token lives at another address,
 * and guessing wrong would flag valid fundings as `invalid`.
 */
const MAINNET_LOCKED_TOKEN: Record<number, `0x${string}`> = {
  1: "0x18084fba666a33d37592fa2633fd49a74dd93a88", // tBTC
  42161: "0x6c84a8f1c29108F47a79964b5Fe888D4f4D0dE40", // tBTC
  137: "0x1BFD67037B42Cf73acF2047067bd4F2C47D9BfD6", // WBTC
};

function lockedTokenFallback(
  chainId: number,
  network: string | undefined,
): `0x${string}` | undefined {
  const net = network?.toLowerCase();
  return net === "bitcoin" || net === "mainnet"
    ? MAINNET_LOCKED_TOKEN[chainId]
    : undefined;
}

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
  arkade_htlc_script_version?: number;
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
    arkadeHtlcScriptVersion: r.arkade_htlc_script_version,
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
  /** The funder / refund address — completes the isActive tuple when known. */
  sender?: string;
  /** Refund timelock in unix seconds, exactly as funded on-chain. */
  timelockSec?: number;
  /** Swap creation time (ISO) — lower-bounds log scans. */
  createdAt?: string;
}): HtlcRef {
  const createdAtMs = args.createdAt ? Date.parse(args.createdAt) : Number.NaN;
  return {
    ledger: "evm",
    chainId: args.chainId,
    htlc: ensure0x(args.htlc),
    preimageHash: ensure0x(args.hashLock),
    claimAddress: ensure0x(args.claimAddress),
    expectedAmount: BigInt(args.expectedSats),
    expectedToken: args.token ? ensure0x(args.token) : undefined,
    sender: args.sender ? ensure0x(args.sender) : undefined,
    timelockSec: args.timelockSec,
    createdAtMs: Number.isNaN(createdAtMs) ? undefined : createdAtMs,
  };
}

/**
 * The Bitcoin HTLC leg. `hashLock` is the SHA-256 preimage hash (`evm_hash_lock`);
 * the on-chain script commits `ripemd160` of it, but the classifier verifies a
 * revealed preimage as `sha256(preimage) === hashLock`.
 */
/**
 * Confirmations a CLIENT-funded Bitcoin HTLC needs before it reads as funded.
 *
 * The server does not act on a client funding until it has a blocktime, so
 * observing it sooner would report the swap as funded while the other side is
 * still waiting for a block. A server-funded leg keeps the reader's 0-conf
 * default, which is what lets a claim go out without waiting ~10 minutes.
 */
const CLIENT_FUNDING_MIN_CONFIRMATIONS = 1;

function bitcoinLeg(
  address: string,
  hashLock: string,
  expectedSats: number,
  minConfirmations?: number,
): HtlcRef {
  return {
    ledger: "bitcoin",
    address,
    preimageHash: strip0x(hashLock),
    expectedSats,
    minConfirmations,
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
          sender: r.server_evm_address, // the server funded it
          timelockSec: r.evm_refund_locktime,
          createdAt: r.created_at,
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
          // evm_to_arkade doesn't expose the locked token — fall back to the
          // per-chain mainnet constant so the isActive tuple is complete.
          token: lockedTokenFallback(r.evm_chain_id, r.network),
          sender: r.client_evm_address, // the client funded it
          timelockSec: r.evm_refund_locktime,
          createdAt: r.created_at,
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
          CLIENT_FUNDING_MIN_CONFIRMATIONS,
        ),
        serverHtlc: evmLeg({
          chainId: r.evm_chain_id,
          htlc: r.evm_htlc_address,
          hashLock: r.evm_hash_lock,
          claimAddress: r.client_evm_address, // the client claims the server's EVM HTLC
          expectedSats: r.evm_expected_sats,
          token: r.wbtc_address,
          sender: r.server_evm_address, // the server funded it
          timelockSec: r.evm_refund_locktime,
          createdAt: r.created_at,
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
          sender: r.client_evm_address, // the client funded it
          timelockSec: r.evm_refund_locktime,
          createdAt: r.created_at,
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
          CLIENT_FUNDING_MIN_CONFIRMATIONS,
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
          arkadeHtlcScriptVersion: r.arkade_htlc_script_version,
        }),
        clientRefundLocktime: ms(r.btc_refund_locktime),
        serverRefundLocktime: ms(r.vhtlc_refund_locktime),
      };

    // ─── Lightning: one on-chain leg, one off-chain LN payment ───────────────

    // Pay-on-Lightning: the client pays a Lightning invoice (off-chain, nothing to
    // watch) and claims an on-chain HTLC. No client-funded leg — the client's
    // claim completes the swap; a hold invoice that never settles auto-unwinds.

    // Client pays the invoice, then claims the server's EVM HTLC.
    case "lightning_to_evm":
      return {
        swapId: r.id,
        serverHtlc: evmLeg({
          chainId: r.evm_chain_id,
          htlc: r.evm_htlc_address,
          hashLock: r.hash_lock,
          claimAddress: r.client_evm_address, // the client claims the server's EVM HTLC
          expectedSats: r.evm_expected_sats,
          token: r.wbtc_address,
          sender: r.server_evm_address, // the server funded it
          timelockSec: r.evm_refund_locktime,
          createdAt: r.created_at,
        }),
        clientRefundLocktime: 0, // no on-chain client leg
        serverRefundLocktime: ms(r.evm_refund_locktime),
      };

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

    // Client funds the EVM HTLC (evm_expected_sats of the BTC-pegged token);
    // the server pays the Lightning invoice off-chain and claims the HTLC —
    // no server leg to watch.
    case "evm_to_lightning":
      return {
        swapId: r.id,
        clientHtlc: evmLeg({
          chainId: r.evm_chain_id,
          htlc: r.evm_htlc_address,
          hashLock: r.hash_lock,
          claimAddress: r.server_evm_address, // the server claims the client's EVM HTLC
          expectedSats: r.evm_expected_sats,
          token: r.wbtc_address,
          sender: r.evm_coordinator_address, // the coordinator locks on the client's behalf
          timelockSec: r.evm_refund_locktime,
          createdAt: r.created_at,
        }),
        clientRefundLocktime: ms(r.evm_refund_locktime),
        serverRefundLocktime: 0, // no on-chain server leg
      };

    // Client funds the Arkade VHTLC (source_amount); the server pays the
    // Lightning invoice off-chain and claims the VHTLC — no server leg to
    // watch.
    case "arkade_to_lightning":
      return {
        swapId: r.id,
        clientHtlc: arkadeLeg(
          r,
          r.arkade_vhtlc_address,
          Number(r.source_amount),
        ),
        clientRefundLocktime: ms(r.vhtlc_refund_locktime),
        serverRefundLocktime: 0, // no on-chain server leg
      };
    default:
      return undefined;
  }
}
