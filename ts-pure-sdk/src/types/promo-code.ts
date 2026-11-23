/**
 * What a promo code gives, from `GET /promo-codes/{code}`. Use it to show the
 * discount and keep the swap under `max_amount_sats` before quoting; pass the
 * code as `promoCode` on the quote and the create to apply it.
 */
export interface PromoCodeResponse {
  code: string;
  /** Base fee with the code, as a decimal fraction (`0.001` = 0.1%). Replaces the pair's fee. */
  fee_rate: number;
  /** Largest swap the code applies to, in sats (BTC leg). */
  max_amount_sats: number;
  /** Swaps that can still use the code. */
  remaining_uses: number;
  /** RFC 3339 expiry. */
  expires_at: string;
}
