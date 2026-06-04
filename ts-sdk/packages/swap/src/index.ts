/**
 * @satora/swap — the Satora-scoped name for the swap client.
 *
 * Re-exports the legacy `@lendasat/lendaswap-sdk-pure` bundle verbatim
 * (Client, ClientBuilder, signer helpers, etc.) so new code can import
 * `@satora/swap` while the published legacy package keeps working. This
 * is the seam for migrating implementation into `@satora/*` later.
 */
export * from "@lendasat/lendaswap-sdk-pure";
