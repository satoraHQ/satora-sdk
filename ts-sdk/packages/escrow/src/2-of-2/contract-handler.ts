import {
  type Contract,
  type ContractHandler,
  contractHandlers,
  type PathContext,
  type PathSelection,
  sequenceToTimelock,
  timelockToSequence,
} from "@arkade-os/sdk";
import { hex } from "@scure/base";
import { type EscrowScriptOptions, EscrowVtxoScript } from "./escrow-script.js";

/** Contract type identifier for the cooperative 2-of-2 escrow. */
export const ESCROW_2OF2_CONTRACT_TYPE = "escrow-2of2";

/** Role a wallet can play in a 2-of-2 escrow contract. */
export type EscrowRole = "seller" | "arbiter";

/**
 * Resolve the wallet's role by matching its x-only pubkey (hex) against the
 * contract's seller/arbiter params. An explicit `context.role` wins.
 *
 * The buyer holds no key in the script, so it is never a role here.
 */
function resolveEscrowRole(
  contract: Contract,
  context: PathContext,
): EscrowRole | undefined {
  if (context.role === "seller" || context.role === "arbiter") {
    return context.role;
  }
  const walletKey = context.walletPubKey?.toLowerCase();
  if (!walletKey) return undefined;
  if (walletKey === contract.params.sellerPubKey?.toLowerCase()) {
    return "seller";
  }
  if (walletKey === contract.params.arbiterPubKey?.toLowerCase()) {
    return "arbiter";
  }
  return undefined;
}

/**
 * CSV maturity check for the escape leaf: is the relative (BIP-68) timelock
 * satisfied for the vtxo under evaluation?
 *
 * TODO: drop this local copy and import the SDK's `isCsvSpendable` once
 * https://github.com/arkade-os/ts-sdk/pull/541 lands and ships in a release —
 * that PR exports this exact helper. Until then we mirror it here.
 */
function isCsvSpendable(
  context: PathContext,
  sequence: number | undefined,
): boolean {
  if (sequence === undefined) return true;
  if (!context.vtxo) return false;
  const timelock = sequenceToTimelock(sequence);
  if (timelock.type === "blocks") {
    if (
      context.blockHeight === undefined ||
      context.vtxo.status.block_height === undefined
    ) {
      return false;
    }
    return (
      context.blockHeight - context.vtxo.status.block_height >=
      Number(timelock.value)
    );
  }
  const blockTime = context.vtxo.status.block_time;
  if (blockTime === undefined) return false;
  return context.currentTime / 1000 - blockTime >= Number(timelock.value);
}

/** nSequence (BIP-68) for the escape leaf, as stored in the contract params. */
function escapeSequence(contract: Contract): number | undefined {
  return contract.params.exitTimelock
    ? Number(contract.params.exitTimelock)
    : undefined;
}

/**
 * ContractHandler for the cooperative 2-of-2 escrow ({@link EscrowVtxoScript}).
 *
 * Spending paths:
 *  - cooperative (leaf A): 3-of-3 [seller, arbiter, Arkade server]. Available whenever
 *    the server cooperates. Returned for ANY role — the witness is completed
 *    by the multi-party signing choreography (see `signEscrowArkTx`), not by a
 *    single wallet. This mirrors how the SDK's VHTLC handler returns its
 *    multi-party `refund`/`claim` leaves.
 *  - escape (leaf B): arbiter-only, after the CSV. The seller has no
 *    unilateral path by design, so `selectPath` returns null for the seller
 *    once collaboration is unavailable.
 */
export const EscrowContractHandler: ContractHandler<
  EscrowScriptOptions,
  EscrowVtxoScript
> = {
  type: ESCROW_2OF2_CONTRACT_TYPE,

  createScript(params) {
    return new EscrowVtxoScript(this.deserializeParams(params));
  },

  serializeParams(params) {
    return {
      sellerPubKey: hex.encode(params.sellerPubKey),
      arbiterPubKey: hex.encode(params.arbiterPubKey),
      arkadeServerPubKey: hex.encode(params.arkadeServerPubKey),
      exitTimelock: timelockToSequence(params.exitTimelock).toString(),
    };
  },

  deserializeParams(params) {
    return {
      sellerPubKey: hex.decode(params.sellerPubKey),
      arbiterPubKey: hex.decode(params.arbiterPubKey),
      arkadeServerPubKey: hex.decode(params.arkadeServerPubKey),
      exitTimelock: sequenceToTimelock(Number(params.exitTimelock)),
    };
  },

  selectPath(script, contract, context) {
    if (context.collaborative) {
      return { leaf: script.cooperativeLeaf() };
    }
    // Unilateral: only the arbiter, only after the CSV elapses.
    if (resolveEscrowRole(contract, context) !== "arbiter") return null;
    const sequence = escapeSequence(contract);
    if (!isCsvSpendable(context, sequence)) return null;
    return { leaf: script.escapeLeaf(), sequence };
  },

  getAllSpendingPaths(script, contract, context) {
    const paths: PathSelection[] = [{ leaf: script.cooperativeLeaf() }];
    // The escape leaf only exists for the arbiter (CSV checked at tx time).
    if (resolveEscrowRole(contract, context) === "arbiter") {
      paths.push({
        leaf: script.escapeLeaf(),
        sequence: escapeSequence(contract),
      });
    }
    return paths;
  },

  getSpendablePaths(script, contract, context) {
    const paths: PathSelection[] = [];
    if (context.collaborative) {
      paths.push({ leaf: script.cooperativeLeaf() });
    }
    if (resolveEscrowRole(contract, context) === "arbiter") {
      const sequence = escapeSequence(contract);
      if (isCsvSpendable(context, sequence)) {
        paths.push({ leaf: script.escapeLeaf(), sequence });
      }
    }
    return paths;
  },
};

/**
 * Register {@link EscrowContractHandler} in the SDK's global handler registry.
 * Idempotent: a no-op if a handler for the type is already registered.
 */
export function registerEscrowContractHandler(): void {
  if (!contractHandlers.has(ESCROW_2OF2_CONTRACT_TYPE)) {
    contractHandlers.register(EscrowContractHandler);
  }
}
