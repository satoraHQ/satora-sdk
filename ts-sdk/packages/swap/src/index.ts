/**
 * @satora/swap — the Satora swap client.
 *
 * Re-exports the legacy `@lendasat/lendaswap-sdk-pure` bundle (types, signer
 * helpers, storage, etc.), but shadows `Client` and `ClientBuilder` with the
 * Satora-native versions from `./client`. Those are drop-in replacements — same
 * public surface, forwarding to an internal legacy client for now — and are the
 * seam for migrating implementation into `@satora/*` and adding new features.
 *
 * Explicit named exports below take precedence over the `export *` star for the
 * same names, so consumers importing `{ Client, ClientBuilder }` get the new
 * ones while everything else stays legacy.
 */
export * from "@lendasat/lendaswap-sdk-pure";
export { Client, ClientBuilder } from "./client.js";
