/**
 * Chain identifier the API speaks.
 *
 * Mirrors the server's `crate::Chain` enum at its current wire encoding.
 * Hand-written rather than re-exported from the OpenAPI types so the SDK
 * surface stays decoupled from codegen — additions land here when the
 * server starts speaking a new chain, and old binaries keep working
 * because the union widens additively.
 */
export type Chain =
  | "Arkade"
  | "Lightning"
  | "Bitcoin"
  // EVM chain ids as strings — matches the wire encoding used by the
  // /quote and /swap-pairs endpoints today.
  | "137" // Polygon
  | "1" // Ethereum
  | "42161"; // Arbitrum
