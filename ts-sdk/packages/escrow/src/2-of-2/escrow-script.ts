import {
  CSVMultisigTapscript,
  MultisigTapscript,
  type Network,
  type RelativeTimelock,
  VtxoScript,
} from "@arkade-os/sdk";

export interface EscrowScriptOptions {
  /** x-only Schnorr pubkey, 32 bytes. The funding party (seller). */
  sellerPubKey: Uint8Array;
  /** x-only Schnorr pubkey, 32 bytes. The escrow arbiter / cosigner. */
  arbiterPubKey: Uint8Array;
  /** x-only Schnorr pubkey, 32 bytes. Arkade server's signer key. */
  arkadeServerPubKey: Uint8Array;
  /**
   * CSV timelock for the arbiter-only unilateral escape leaf — the
   * Arkade server-mandated unilateral-exit closure. Must be ≥ the Arkade server's
   * `unilateralExitDelay` (~2 days on mutinynet, ~30 days on
   * mainnet) or `submitTx` rejects the script with
   * INVALID_VTXO_SCRIPT "exit delay is too short".
   */
  exitTimelock: RelativeTimelock;
}

/**
 * Two-leaf VtxoScript for a cooperative 2-of-2 escrow on Arkade.
 *
 *   A — cooperative release     : 3-of-3 [seller, arbiter, Arkade server] (no CSV)
 *   B — arbiter unilateral exit : [arbiter] alone after a long CSV
 *
 * The two policy parties are the seller and the arbiter; both must sign
 * the cooperative release. The Arkade server is added to leaf A as required
 * by Arkade round/forfeit semantics — the Arkade server must cosign every
 * cooperative VTXO spend — so at the script level leaf A is 3-of-3 even
 * though only two parties hold policy.
 *
 * The seller has NO unilateral exit. Seller safety relies on a
 * pre-signed cooperative refund Arkade transaction (created at funding time, held by
 * the arbiter). After the CSV elapses, only the arbiter can sweep.
 */
export class EscrowVtxoScript extends VtxoScript {
  readonly options: EscrowScriptOptions;

  constructor(options: EscrowScriptOptions) {
    const cooperativeLeaf = MultisigTapscript.encode({
      pubkeys: [
        options.sellerPubKey,
        options.arbiterPubKey,
        options.arkadeServerPubKey,
      ],
    });

    const escapeLeaf = CSVMultisigTapscript.encode({
      pubkeys: [options.arbiterPubKey],
      timelock: options.exitTimelock,
    });

    super([cooperativeLeaf.script, escapeLeaf.script]);

    this.options = options;
  }

  /** The cooperative-release tapleaf (3-of-3). Index 0. */
  cooperativeLeaf() {
    return this.leaves[0];
  }

  /** The arbiter-only unilateral exit tapleaf (long CSV). Index 1. */
  escapeLeaf() {
    return this.leaves[1];
  }

  /**
   * SDK-conformant alias for {@link cooperativeLeaf}.
   *
   * The Arkade wallet/contract machinery (`deriveContractTapscripts`) expects
   * every contract VtxoScript to expose `forfeit()` as its collaborative
   * forfeit/intent leaf. For this escrow that is the cooperative 3-of-3 leaf.
   */
  forfeit() {
    return this.cooperativeLeaf();
  }

  /**
   * SDK-conformant alias for {@link escapeLeaf}, matching the `DefaultVtxo`
   * `exit()` convention for the unilateral (CSV) leaf.
   */
  exit() {
    return this.escapeLeaf();
  }

  /** Encoded Arkade address (bech32m) for funding this escrow. */
  arkAddress(network: Network): string {
    return this.address(network.hrp, this.options.arkadeServerPubKey).encode();
  }
}
