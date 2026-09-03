/**
 * The derived "what should the client do next?" model.
 *
 * `SwapStatus` says what *happened*; `SwapActions` says what the client *can or
 * should do*. Actions are produced by the pure resolver {@link deriveSwapActions}
 * from a plain {@link SwapActionInput} snapshot — no I/O, so the whole decision
 * matrix is testable with mocked states.
 */
import type { SwapStatus } from "@lendasat/lendaswap-sdk-pure";

/**
 * How the background worker may run an action — the load-bearing safety field.
 * `fund` is never `auto` (spends new money); claim / cctp-mint completion are
 * safe to `auto` (they protect or continue existing funds); refunds are
 * `confirm` (the user should know).
 */
export type SwapActionAutomation = "auto" | "confirm" | "manual";

/** Typed reasons an action can't run yet — no free strings. */
export type BlockedReason = "timelock_not_expired";

export type BlockedBy = {
  kind: BlockedReason;
  message: string;
};

/** Fields common to every action variant. */
type SwapActionBase = {
  /** The SDK's suggested pick among the set. Exactly one action is recommended. */
  recommended: boolean;
  automation: SwapActionAutomation;
  /** Human-readable why — for UI copy and worker logs. */
  reason: string;
  /**
   * Why the action can't run yet. Absent ⟺ the action is runnable now; present
   * ⟺ it's a valid-but-blocked option the UI can surface (e.g. "refund after the
   * timelock"). The recommended action is always runnable (no `blockedBy`).
   */
  blockedBy?: BlockedBy;
};

/** Strongly-typed evidence, only where an action needs it. */
export type CctpClaimEvidence = {
  burnTxHash: `0x${string}`;
  attestationAvailable: boolean;
};

/**
 * What a `wait` is waiting FOR — machine-readable, so a consumer can pick the
 * right surface (invoice page, "deposit seen…", processing spinner, refund
 * countdown) by switching on this field instead of re-deriving it from
 * status + observations.
 *
 * One caveat: `client_payment` covers the whole Lightning-funded window — the
 * chain cannot see whether the invoice was already paid (the server is the
 * only witness for that split), so a consumer that wants "invoice shown" vs
 * "payment in flight" as separate screens still refines this one value with
 * the server status.
 */
export type WaitingOn =
  /** A Lightning payment to be made / locked in as a funded HTLC. */
  | "client_payment"
  /** The client's funding tx was seen and is awaiting confirmation. */
  | "client_funding_confirmation"
  /** The client funded; the server's HTLC hasn't appeared yet. */
  | "server_funding"
  /** The client's claim is in flight, awaiting confirmation. */
  | "claim_confirmation"
  /** Nothing will progress — waiting for the client refund timelock to pass. */
  | "refund_timelock";

/**
 * How a finished swap ended — so a consumer can render success vs refund vs
 * expiry from the terminal `none` action without interpreting `SwapStatus`.
 */
export type SwapOutcome = "completed" | "refunded" | "expired";

// One variant per id. Evidence is required exactly where it applies, absent
// elsewhere — accessing `.evidence` without narrowing on `id` is a type error.
export type WaitAction = SwapActionBase & { id: "wait"; waitingOn: WaitingOn };
export type FundAction = SwapActionBase & { id: "fund" };
export type ClaimAction = SwapActionBase & { id: "claim" };
export type RefundCollaborativeAction = SwapActionBase & {
  id: "refund_collaborative";
};
/** Reclaim the deposit yourself once the timelock passes — venue-agnostic (BTC L1, EVM, or Arkade). */
export type RefundUnilateralAction = SwapActionBase & {
  id: "refund_unilateral";
};
export type RecoverCctpClaimAction = SwapActionBase & {
  id: "recover_cctp_claim";
  evidence: CctpClaimEvidence;
};
export type NoneAction = SwapActionBase & { id: "none"; outcome: SwapOutcome };

/** A single next-action option. Discriminated union on `id`. */
export type SwapAction =
  | WaitAction
  | FundAction
  | ClaimAction
  | RefundCollaborativeAction
  | RefundUnilateralAction
  | RecoverCctpClaimAction
  | NoneAction;

/** Every valid action id — derived from the union so it can't drift. */
export type SwapActionId = SwapAction["id"];

/** The resolver's result: what happened is `status`; what to do is here. */
export type SwapActions = {
  /** id of the recommended action, if any; convenience pointer into `actions`. */
  recommended?: SwapActionId;
  actions: SwapAction[];
  /**
   * The raw chain observations these actions were derived from (set by the
   * tracker; absent from a bare `deriveSwapActions` call). Chain FACTS — which
   * leg is funded/spent — not protocol states: lets a UI render progress from
   * chain truth (even when the server is stale or unreachable) without ever
   * interpreting the `SwapStatus` enum.
   */
  observations?: SwapObservations;
};

/**
 * The pure input to {@link deriveSwapActions}: a plain, fully-resolved snapshot.
 *
 * The resolver reads nothing ambient — `now` is injected (never `Date.now()`),
 * and every environment fact is a field here. Grows as more states are modelled;
 * unknown/omitted live facts make the resolver return a provisional or blocked
 * action rather than reaching out for them.
 */
export type SwapActionInput = {
  /**
   * The swap's lifecycle status. Trusted source is the chain-derived status from
   * {@link deriveSwapStatus}; the server's status is only a hint the wrapper
   * cross-checks against it.
   */
  status: SwapStatus;
  /**
   * Current time (ms) on the chain holding the CLIENT's HTLC — the chain's own
   * clock used to evaluate its timelock, NOT wall-clock: MTP (median-time-past)
   * for Bitcoin/Arkade, block.timestamp for EVM, invoice-clock for Lightning.
   * Injected so the resolver stays deterministic. (A wall-clock approximation
   * works for a UI hint, but the chain's clock is what actually gates the swap.)
   */
  clientChainNow: number;
  /** As {@link clientChainNow}, for the chain holding the SERVER's HTLC. */
  serverChainNow: number;
  /**
   * Timelock (ms) on the client's HTLC, evaluated against {@link clientChainNow}.
   * Bounds funding (pending: fund before it) and unlocks the unilateral refund
   * (clientfunded: refund after it) — the same locktime, two readings.
   */
  clientRefundLocktime: number;
  /**
   * Timelock (ms) on the server's HTLC, evaluated against {@link serverChainNow}
   * — the client's deadline to claim. Past it the server may reclaim its funds,
   * so claiming becomes a race.
   */
  serverRefundLocktime: number;
  /**
   * Whether the client's deposit is an on-chain HTLC it funds itself (the
   * default). `false` for pay-on-Lightning swaps (`lightning_to_*`): there the
   * client "funds" by paying a Lightning invoice off-chain, so there is nothing
   * for us to recommend funding and nothing to unilaterally refund — the
   * Lightning wallet auto-unwinds a hold invoice that never settles. When
   * `false`, `pending` resolves to `wait` (not `fund`) and no refund is offered.
   */
  clientFunds?: boolean;
  /**
   * Whether the server's leg is an on-chain HTLC the client claims (the
   * default). `false` for receive-on-Lightning swaps (`*_to_lightning`): there
   * the server "funds" by paying the client's invoice off-chain, so there is
   * nothing to claim and no claim window — {@link serverRefundLocktime} is
   * meaningless. When `false`, `serverfunded` (the payment is in flight)
   * resolves to `wait`, with the unilateral refund listed but blocked until
   * the client's own timelock, exactly like `clientfunded`.
   */
  serverFunds?: boolean;
};

/** On-chain state of one HTLC in a swap. */
export type HtlcObservation =
  | "absent" // not funded
  | "mempool" // funding tx seen, unconfirmed
  | "confirmed" // funded + confirmed with the expected amount/terms
  | "invalid" // funded, but not on the swap's terms (e.g. under the expected amount)
  | "spent_claim" // swept via the preimage
  | "spent_refund"; // reclaimed via the timelock

/**
 * On-chain facts about a swap's two HTLCs, from the client's own observation (no
 * server trust). `clientHtlc` is the HTLC the client funded; `serverHtlc` the one
 * the server funded and the client claims.
 *
 * Spend semantics differ per side: on `clientHtlc`, `spent_claim` = the server
 * swept it with the revealed preimage and `spent_refund` = the client reclaimed
 * its deposit; on `serverHtlc`, `spent_claim` = the client redeemed and
 * `spent_refund` = the server reclaimed its funds.
 *
 * One leg may be `undefined` for Lightning swaps. These are Boltz submarine swaps
 * between an Arkade VHTLC and Lightning, so the "Lightning side" is itself realized
 * on-chain as an Arkade VHTLC (`boltz_vhtlc_address`, or reconstructed for the EVM
 * directions) — except `lightning_to_arkade`, whose Boltz reverse swap outputs the
 * very VHTLC the client claims. That Boltz-side VHTLC is observable but not tracked
 * yet: the client never funds or claims it, so it doesn't change the next action.
 * We watch only the single leg the client funds or claims:
 * - `serverHtlc` undefined (receive-on-LN, `*_to_lightning`): the client funds
 *   the on-chain leg and receives via Lightning; the server sweeping the
 *   client's leg (`spent_claim`) is the completion signal.
 * - `clientHtlc` undefined (pay-on-LN, `lightning_to_*`): the client pays a
 *   Lightning invoice and claims the on-chain leg; that claim completes the swap.
 */
export type SwapObservations = {
  clientHtlc?: HtlcObservation;
  serverHtlc?: HtlcObservation;
};
