/**
 * Derive a swap's lifecycle status from on-chain observations alone — the
 * trusted, server-independent source of truth.
 *
 * Pure: `deriveSwapStatus(observations) -> SwapStatus | undefined`. The wrapper
 * gathers the {@link SwapObservations} from chain (the server's status is only a
 * hint used to fetch efficiently), then cross-checks: if this disagrees with the
 * server, act on this. If the server is unavailable, derive from the local
 * recovery bundle's HTLC addresses — recovery still works.
 *
 * Returns `undefined` for contradictory/indeterminate observations (e.g. the
 * server's HTLC swept-by-claim while the client's HTLC was never involved — the
 * server can't hold the preimage before the client reveals it).
 *
 * Scope: this maps *settled* HTLC state to a status. Time- or param-dependent
 * refinements aren't derivable from HTLC state alone and are layered on
 * afterwards (or collapse into the state here):
 * - `expired` / `clientfundedtoolate`: `pending`/`clientfunded` + a clock check.
 * - `clientinvalidfunded`: needs the expected HTLC params, not just "confirmed".
 * - `serverwontfund`: a server intention, invisible on-chain — acts like
 *   `clientfunded` (refund after the timelock), so it never needs deriving.
 * - `clientredeeming`: an in-flight (mempool) claim — a transient, not modelled
 *   from settled state here.
 */
import type { SwapStatus } from "@lendasat/lendaswap-sdk-pure";
import type { HtlcObservation, SwapObservations } from "./types.js";

export function deriveSwapStatus(
  obs: SwapObservations,
): SwapStatus | undefined {
  const { clientHtlc, serverHtlc } = obs;

  // Lightning swaps watch a single on-chain leg; the other side is an off-chain
  // Lightning payment with no HTLC (see SwapObservations). Derive from the leg
  // that exists.
  if (serverHtlc === undefined && clientHtlc !== undefined) {
    return receiveOnLightningStatus(clientHtlc); // *_to_lightning
  }
  if (clientHtlc === undefined && serverHtlc !== undefined) {
    return payOnLightningStatus(serverHtlc); // lightning_to_*
  }
  if (clientHtlc === undefined || serverHtlc === undefined) return undefined;

  // A spend on either HTLC has resolved (part of) the swap — check these first.
  if (clientHtlc === "spent_claim" && serverHtlc === "spent_claim") {
    return "serverredeemed"; // client redeemed, server swept — fully complete
  }
  if (clientHtlc === "spent_refund" && serverHtlc === "spent_claim") {
    return "clientredeemedandclientrefunded"; // client took both sides (anomaly)
  }
  if (clientHtlc === "spent_refund" && serverHtlc === "spent_refund") {
    return "clientrefundedserverrefunded";
  }
  if (clientHtlc === "spent_refund" && serverHtlc === "confirmed") {
    return "clientrefundedserverfunded"; // error state per the backend
  }
  if (
    clientHtlc === "spent_refund" &&
    (serverHtlc === "absent" || serverHtlc === "mempool")
  ) {
    return "clientrefunded";
  }
  if (clientHtlc === "confirmed" && serverHtlc === "spent_claim") {
    return "clientredeemed"; // client claimed the server's HTLC
  }
  if (clientHtlc === "confirmed" && serverHtlc === "spent_refund") {
    return "clientfundedserverrefunded"; // server let its HTLC time out
  }

  // Client funded, but not on the swap's terms (e.g. under the expected amount) —
  // the swap won't complete honestly, so reclaim the deposit.
  if (clientHtlc === "invalid") return "clientinvalidfunded";

  // No relevant spend — funding progression. `serverfunded` requires the server's
  // HTLC to be `confirmed` (correct amount/terms); an `invalid` server funding is
  // deliberately NOT serverfunded, so the client never claims a bad server leg —
  // it stays `clientfunded` and the client refunds after its timelock.
  if (clientHtlc === "absent") return "pending";
  if (clientHtlc === "mempool") return "clientfundingseen";
  if (clientHtlc === "confirmed") {
    return serverHtlc === "confirmed" ? "serverfunded" : "clientfunded";
  }

  // Contradictory (e.g. the client's HTLC swept-by-claim while the server's HTLC
  // isn't also claimed — impossible without the preimage having been revealed).
  return undefined;
}

/**
 * Receive-on-Lightning (`arkade_to_lightning`, `evm_to_lightning`): the client
 * funds a single on-chain HTLC and receives via Lightning. There is no
 * client-claimed server leg — the server sweeping the client's HTLC with the
 * preimage (it obtained by paying the client's Lightning invoice) is the success
 * signal. So the whole lifecycle derives from the client's leg alone.
 */
function receiveOnLightningStatus(clientHtlc: HtlcObservation): SwapStatus {
  switch (clientHtlc) {
    case "spent_claim":
      return "serverredeemed"; // server swept the deposit ⟹ it paid the invoice
    case "spent_refund":
      return "clientrefunded";
    case "invalid":
      return "clientinvalidfunded";
    case "confirmed":
      return "clientfunded"; // funded; waiting for the server to pay + sweep
    case "mempool":
      return "clientfundingseen";
    case "absent":
      return "pending";
  }
}

/**
 * Pay-on-Lightning (`lightning_to_arkade`, `lightning_to_evm`): the client pays a
 * Lightning invoice (off-chain, nothing for us to watch) and claims a single
 * on-chain HTLC. That claim completes the swap. The client has no on-chain
 * deposit at risk, so there is never a refund — a hold invoice that never settles
 * is auto-cancelled by the Lightning wallet.
 */
function payOnLightningStatus(serverHtlc: HtlcObservation): SwapStatus {
  switch (serverHtlc) {
    case "spent_claim":
      return "clientredeemed"; // client claimed the on-chain leg ⟹ complete
    case "confirmed":
      return "serverfunded"; // ready to claim
    case "spent_refund":
      // The server reclaimed its leg — the swap failed; the client's Lightning
      // payment unwinds on its own. Terminal, nothing on-chain for the client.
      return "clientrefunded";
    case "invalid":
    case "mempool":
    case "absent":
      // Nothing (correct) to claim yet — keep waiting for the invoice to land as a
      // funded HTLC. `invalid` (wrong amount/terms) also stays a wait: the client
      // must not claim a bad leg, and the Lightning payment will unwind.
      return "pending";
  }
}
