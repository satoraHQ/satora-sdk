import type {
  ClaimAction,
  FundAction,
  NoneAction,
  RefundUnilateralAction,
  SwapActionInput,
  SwapActions,
  WaitAction,
} from "./types.js";

/**
 * How long past the client refund locktime a `pending` swap keeps being watched
 * before it is declared dead. Covers observation lag on a deadline-straddling
 * funding tx (EVM has no mempool-level observation, and clocks are partly
 * extrapolated); one that stays invisible longer than this is treated as never
 * sent. A later `startTracking` still picks the swap up from storage if a
 * funding somehow surfaces afterwards.
 */
export const FUNDING_REAP_GRACE_MS = 60 * 60 * 1000;

export function deriveSwapActions(input: SwapActionInput): SwapActions {
  const {
    status,
    clientChainNow,
    serverChainNow,
    clientRefundLocktime,
    serverRefundLocktime,
    clientFunds = true,
    serverFunds = true,
  } = input;

  switch (status) {
    case "pending": {
      // Pay-on-Lightning: the client's "funding" is paying the invoice off-chain
      // (handled by their Lightning wallet), not an on-chain deposit — there is
      // nothing for us to recommend funding, so just wait for the invoice to land
      // as a funded HTLC we can claim.
      if (!clientFunds) {
        const wait: WaitAction = {
          id: "wait",
          waitingOn: "client_payment",
          recommended: true,
          automation: "auto",
          reason: "Waiting for the Lightning payment to be locked in.",
        };
        return { recommended: "wait", actions: [wait] };
      }

      // Funding is "too late" once past the client's refund locktime (the funded
      // deposit would be immediately refundable). But `pending` only means the
      // last chain read saw NO deposit — a funding broadcast near the deadline
      // can still surface after it (and would need the refund path). So keep
      // watching through a grace window; only once it has clearly stayed unfunded
      // derive `none`, letting the tracker reap the dead swap instead of
      // watching it forever.
      if (clientChainNow >= clientRefundLocktime + FUNDING_REAP_GRACE_MS) {
        const done: NoneAction = {
          id: "none",
          outcome: "expired",
          recommended: true,
          automation: "auto",
          reason: "The funding window closed.",
        };
        return { recommended: "none", actions: [done] };
      }
      if (clientChainNow >= clientRefundLocktime) return { actions: [] };

      const fund: FundAction = {
        id: "fund",
        recommended: true,
        // Funding spends new user money, so it is never auto-run.
        automation: "manual",
        reason: "Send the deposit to start the swap.",
      };
      return { recommended: "fund", actions: [fund] };
    }

    case "clientfundingseen": {
      // Funding tx seen but not yet confirmed — wait for confirmation (then the
      // server funds its side). Nothing to refund until the HTLC confirms.
      const wait: WaitAction = {
        id: "wait",
        waitingOn: "client_funding_confirmation",
        recommended: true,
        automation: "auto",
        reason: "Waiting for your funding transaction to confirm.",
      };
      return { recommended: "wait", actions: [wait] };
    }

    case "clientfunded": {
      // Pay-on-Lightning: "funded" means the Lightning payment is locked in —
      // there is no on-chain deposit and thus no refund to offer. Wait for the
      // server to fund its leg; a payment that never settles unwinds on its own.
      if (!clientFunds) {
        const wait: WaitAction = {
          id: "wait",
          waitingOn: "server_funding",
          recommended: true,
          automation: "auto",
          reason: "Waiting for the server to fund the swap.",
        };
        return { recommended: "wait", actions: [wait] };
      }

      const refundUnlocked = clientChainNow >= clientRefundLocktime;

      if (refundUnlocked) {
        // Timelock passed: reclaiming the deposit is the recommended way out.
        const refund: RefundUnilateralAction = {
          id: "refund_unilateral",
          recommended: true,
          automation: "confirm",
          reason: "Reclaim your deposit.",
        };
        return { recommended: "refund_unilateral", actions: [refund] };
      }

      // Still within the timelock: wait for the server; refund is a valid but
      // not-yet-runnable option, surfaced so the UI can show when it unlocks.
      const wait: WaitAction = {
        id: "wait",
        waitingOn: "server_funding",
        recommended: true,
        automation: "auto",
        reason: "Waiting for the server to fund the swap.",
      };
      const refund: RefundUnilateralAction = {
        id: "refund_unilateral",
        recommended: false,
        automation: "confirm",
        reason: "Refund becomes available once the timelock passes.",
        blockedBy: {
          kind: "timelock_not_expired",
          message: "The refund timelock has not passed yet.",
        },
      };
      return { recommended: "wait", actions: [wait, refund] };
    }

    case "serverfunded": {
      // Receive-on-Lightning: "server funded" means the payment to the client's
      // invoice is in flight — there is no server HTLC to claim and no claim
      // window. Wait for it to settle (the server then sweeps the client's leg,
      // which completes the swap); the deposit stays refundable at the client's
      // own timelock, as in `clientfunded`.
      if (!serverFunds) {
        const refundUnlocked = clientChainNow >= clientRefundLocktime;
        if (refundUnlocked) {
          const refund: RefundUnilateralAction = {
            id: "refund_unilateral",
            recommended: true,
            automation: "confirm",
            reason: "Reclaim your deposit.",
          };
          return { recommended: "refund_unilateral", actions: [refund] };
        }
        const wait: WaitAction = {
          id: "wait",
          waitingOn: "server_funding",
          recommended: true,
          automation: "auto",
          reason: "Waiting for the Lightning payment to settle.",
        };
        const refund: RefundUnilateralAction = {
          id: "refund_unilateral",
          recommended: false,
          automation: "confirm",
          reason: "Refund becomes available once the timelock passes.",
          blockedBy: {
            kind: "timelock_not_expired",
            message: "The refund timelock has not passed yet.",
          },
        };
        return { recommended: "wait", actions: [wait, refund] };
      }

      // The client created the swap, so it always holds the preimage.
      const claimWindowClosed = serverChainNow >= serverRefundLocktime;

      if (!claimWindowClosed) {
        // Server can't refund yet, so revealing the preimage by claiming is safe.
        // Claiming secures the user's funds, so it is safe to auto-run.
        const claim: ClaimAction = {
          id: "claim",
          recommended: true,
          automation: "auto",
          reason: "Redeem the swap with your preimage.",
        };
        return { recommended: "claim", actions: [claim] };
      }

      // Pay-on-Lightning: the client has no on-chain deposit at risk, so there is
      // nothing to protect by keeping the secret and nothing to refund. Once the
      // claim window closed the leg is (being) reclaimed by the server; the
      // client's Lightning payment unwinds on its own. Terminal.
      if (!clientFunds) {
        const done: NoneAction = {
          id: "none",
          outcome: "refunded",
          recommended: true,
          automation: "auto",
          reason:
            "The claim window passed — your Lightning payment will be refunded automatically.",
        };
        return { recommended: "none", actions: [done] };
      }

      // Claim window closed: the server can now refund its HTLC, and claiming
      // would reveal the preimage — letting the server also take the client's
      // deposit. Never claim now; refund instead (keeping the secret). Refund
      // unlocks at the client's own, later timelock.
      const refundUnlocked = clientChainNow >= clientRefundLocktime;

      if (refundUnlocked) {
        const refund: RefundUnilateralAction = {
          id: "refund_unilateral",
          recommended: true,
          automation: "confirm",
          reason:
            "Reclaim your deposit — do not claim, as revealing the preimage would let the server take it.",
        };
        return { recommended: "refund_unilateral", actions: [refund] };
      }

      // Refund not unlocked yet: wait for the client timelock, then refund. Still
      // must not claim.
      const wait: WaitAction = {
        id: "wait",
        waitingOn: "refund_timelock",
        recommended: true,
        automation: "auto",
        reason:
          "Wait for your refund timelock — do not claim, as revealing the preimage would let the server take your deposit.",
      };
      const refund: RefundUnilateralAction = {
        id: "refund_unilateral",
        recommended: false,
        automation: "confirm",
        reason: "Refund becomes available once the timelock passes.",
        blockedBy: {
          kind: "timelock_not_expired",
          message: "The refund timelock has not passed yet.",
        },
      };
      return { recommended: "wait", actions: [wait, refund] };
    }

    case "clientredeeming": {
      // The client's claim (redeem) is in flight — wait for it to confirm, then
      // the funds are received.
      const wait: WaitAction = {
        id: "wait",
        waitingOn: "claim_confirmation",
        recommended: true,
        automation: "auto",
        reason: "Redeeming your funds — waiting for the claim to confirm.",
      };
      return { recommended: "wait", actions: [wait] };
    }

    case "clientredeemed":
    case "serverredeemed": {
      // The client claimed its funds — done from the client's side. The server
      // will sweep the client's deposit with the now-public preimage (and
      // `serverredeemed` means it already has); the client must not try to refund
      // it. Both are terminal successes — nothing to do.
      const done: NoneAction = {
        id: "none",
        outcome: "completed",
        recommended: true,
        automation: "auto",
        reason: "Swap complete — you've received your funds.",
      };
      return { recommended: "none", actions: [done] };
    }

    case "expired": {
      // "Pending → Expired": the funding window passed without the client
      // funding, so nothing is locked and there is nothing to refund. Terminal.
      const done: NoneAction = {
        id: "none",
        outcome: "expired",
        recommended: true,
        automation: "auto",
        reason: "The swap expired without funding — nothing was locked.",
      };
      return { recommended: "none", actions: [done] };
    }

    case "clientrefunded":
    case "clientrefundedserverrefunded":
    case "clientrefundedserverfunded": {
      // Terminal: the client's deposit was refunded. (`*serverrefunded` also
      // returned the server's; `clientrefundedserverfunded` is the backend's
      // "should never occur" error where the server funded after the client
      // refunded — the server's problem, not the client's.) Nothing to do.
      const done: NoneAction = {
        id: "none",
        outcome: "refunded",
        recommended: true,
        automation: "auto",
        reason: "Refunded — your deposit was returned.",
      };
      return { recommended: "none", actions: [done] };
    }

    case "clientredeemedandclientrefunded": {
      // Error state: the client both redeemed and refunded (took both sides).
      // Anomalous but terminal — nothing for the client to do.
      const done: NoneAction = {
        id: "none",
        // The client holds both sides' funds — "completed" from its viewpoint.
        outcome: "completed",
        recommended: true,
        automation: "auto",
        reason:
          "This swap ended in an anomalous state (redeemed and refunded); no action needed.",
      };
      return { recommended: "none", actions: [done] };
    }

    case "clientfundedserverrefunded":
    case "serverwontfund":
    case "clientfundedtoolate":
    case "clientinvalidfunded": {
      // Pay-on-Lightning: no on-chain deposit at risk — the failed swap's
      // Lightning payment unwinds on its own. Terminal.
      if (!clientFunds) {
        const done: NoneAction = {
          id: "none",
          outcome: "refunded",
          recommended: true,
          automation: "auto",
          reason:
            "The swap did not complete — your Lightning payment will be refunded automatically.",
        };
        return { recommended: "none", actions: [done] };
      }

      // The swap failed with the client's deposit funded — the only path is to
      // reclaim it via the client's refund timelock. (No happy-path wait: unlike
      // `clientfunded`, the swap will not complete.)
      const refundUnlocked = clientChainNow >= clientRefundLocktime;

      if (refundUnlocked) {
        const refund: RefundUnilateralAction = {
          id: "refund_unilateral",
          recommended: true,
          automation: "confirm",
          reason: "Reclaim your deposit — the swap did not complete.",
        };
        return { recommended: "refund_unilateral", actions: [refund] };
      }

      // Refund not unlocked yet: wait for the client timelock, then refund.
      const wait: WaitAction = {
        id: "wait",
        waitingOn: "refund_timelock",
        recommended: true,
        automation: "auto",
        reason:
          "The swap did not complete; waiting for the refund timelock to reclaim your deposit.",
      };
      const refund: RefundUnilateralAction = {
        id: "refund_unilateral",
        recommended: false,
        automation: "confirm",
        reason: "Refund becomes available once the timelock passes.",
        blockedBy: {
          kind: "timelock_not_expired",
          message: "The refund timelock has not passed yet.",
        },
      };
      return { recommended: "wait", actions: [wait, refund] };
    }

    default:
      return assertNever(status);
  }
}

/** Exhaustiveness guard: a compile error here means a `SwapStatus` case is missing. */
function assertNever(status: never): never {
  throw new Error(`unhandled swap status: ${String(status)}`);
}
