// One-stop import: re-export all escrow primitives so consumers need only
// `@satora/escrow-client` (plus the swap Client they already run).
export * from "@satora/escrow";
// High-level escrow flows (EscrowClient).
export * from "./escrow-client.js";
