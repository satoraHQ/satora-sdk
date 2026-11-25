/** Test networks use short HTLCs; production needs a larger first-disclosure margin. */
export function assertPaymentClaimWindow(
  network: string,
  refundLocktime: number,
  alreadySubmitted = false,
  now = Math.floor(Date.now() / 1000),
): void {
  if (!Number.isFinite(refundLocktime))
    throw new Error("Invalid Bitcoin refund expiry");
  if (
    !alreadySubmitted &&
    refundLocktime - now < (network === "mainnet" ? 600 : 30)
  )
    throw new Error(
      "Too close to Bitcoin refund expiry to safely reveal the payment preimage",
    );
}
