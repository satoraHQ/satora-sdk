/**
 * `ReferralFeeResponse` — the referral / extra-fee delta for a referral code.
 *
 * Hand-written; identity-shaped with the OpenAPI codegen today. The fourth
 * quote-composition primitive: `compose` adds `extra_fee_rate` on top of the
 * per-pair `fee_percentage` from `/swap-pairs`.
 */

export interface ReferralFeeResponse {
  /**
   * Extra fee as a **decimal fraction** (e.g. `0.005` = 0.5% = 50 bps), added
   * on top of the per-pair `fee_percentage` (also a fraction) — compose as
   * `fee_percentage + extra_fee_rate`, no conversion. `0` when there's
   * no/unknown referral and no `extraFees`. (The input `extraFees` is in bps;
   * this output is a fraction — the server converts once.)
   */
  extra_fee_rate: number;
  /** Partner label for the referral code, when registered. Informational. */
  referral_label?: string;
}

export interface WireReferralFeeResponse {
  extra_fee_rate: number;
  referral_label?: string | null;
}

export function fromWireReferralFeeResponse(
  wire: WireReferralFeeResponse,
): ReferralFeeResponse {
  return {
    extra_fee_rate: wire.extra_fee_rate,
    referral_label: wire.referral_label ?? undefined,
  };
}
