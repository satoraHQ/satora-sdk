/**
 * The promo code a quote was priced with, on `QuoteResponse.promo` and
 * `ReferralFeeResponse.promo`. The quote's `protocol_fee_rate` and
 * `max_amount` already include it.
 */
export interface AppliedPromo {
  code: string;
  /**
   * The promo's base fee as a decimal fraction (`0.001` = 0.1%). The swap's
   * base fee is the lower of it and the route's; any extra fee is added on top.
   */
  fee_rate: number;
  /** Largest swap the code applies to, in sats. Lifts the route maximum when higher. */
  max_amount_sats: number;
}
