/**
 * Public (DTO) chain identifier the SDK surface speaks — a **superset** of the
 * server's wire enum ({@link WireChain}).
 *
 * The compose layer accepts these on quote inputs and in the bridge-token
 * lists, then converts down to a {@link WireChain} before calling the server:
 * a CCTP/OFT bridge target (e.g. Base, Optimism, Solana) is remapped to the
 * Arbitrum hub with the destination carried separately in `bridge_target_chain`.
 * The server's `crate::Chain` enum itself stays narrow, so this must NOT be
 * used where a value is sent on the wire — use {@link WireChain} there.
 *
 * Hand-written rather than re-exported from codegen so the SDK surface stays
 * decoupled; the union widens additively as new bridge chains land.
 */
export type Chain =
  | "Arkade"
  | "Lightning"
  | "Bitcoin"
  | "Solana"
  // EVM chain ids as strings — the wire encoding used by /quote and
  // /swap-pairs. Hub chains the backend prices natively:
  | "1" // Ethereum
  | "137" // Polygon
  | "42161" // Arbitrum
  // CCTP bridge chains (mirrors `CCTP_DOMAINS`): valid quote targets, and
  // valid USDC sources via CCTP-inbound (`bridge_source_chain`).
  | "10" // Optimism
  | "8453" // Base
  | "43114" // Avalanche
  | "130" // Unichain
  | "59144" // Linea
  | "146" // Sonic
  | "480" // World Chain
  | "143" // Monad
  | "1329" // Sei
  | "50" // XDC
  | "999" // HyperEVM
  | "57073"; // Ink

/**
 * The chain identifiers the **server** actually speaks on the wire — mirrors
 * `crate::Chain` and the generated `components["schemas"]["Chain"]`. Bridge
 * targets never appear here; they're remapped to a hub before the request and
 * carried in `bridge_target_chain`. Use this for anything sent to / received
 * from the server; use {@link Chain} for the SDK's public surface.
 */
export type WireChain =
  | "Arkade"
  | "Lightning"
  | "Bitcoin"
  | "1" // Ethereum
  | "137" // Polygon
  | "42161"; // Arbitrum
