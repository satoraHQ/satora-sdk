// One-stop import: re-export all escrow primitives so consumers need only
// `@satora/escrow-client` (plus the swap Client they already run).
export * from "@satora/escrow";
// Destination classification used by the smart `withdraw` (useful for routing
// or labelling a destination in a UI before calling it).
export { classifyDestination, type DestinationKind } from "./destination.js";
// High-level escrow flows (EscrowClient).
export * from "./escrow-client.js";
