/**
 * Protocol compatibility epochs, one per protocol component.
 *
 * A swap direction is a pair of legs, and what breaks compatibility is a leg's
 * contract or script, not the pairing. Each component has its own integer
 * epoch and request header. The SDK sends every component header on every
 * request; the server checks the ones the swap in question uses.
 *
 * Mirrors `swap/src/protocol_compat.rs` on the server; keep the two in sync
 * and document what changed whenever a new epoch is added.
 */

export const ProtocolComponent = {
  /** On-chain Bitcoin HTLC script. */
  BitcoinHtlc: "bitcoin-htlc",
  /** Arkade VHTLC script and the delegate settlement flow. */
  ArkadeVhtlc: "arkade-vhtlc",
  /** Lightning leg: hold invoices, quotes, preimage handling. */
  Lightning: "lightning",
  /** HTLCErc20 and the HTLCCoordinator in front of it. */
  EvmErc20Htlc: "evm-erc20-htlc",
  /** HTLCNative for chains whose swap asset is the native coin. */
  EvmNativeHtlc: "evm-native-htlc",
} as const;

export type ProtocolComponent =
  (typeof ProtocolComponent)[keyof typeof ProtocolComponent];

export const ProtocolEpoch = {
  /**
   * Baseline epoch: the wire protocol as served by API `0.3.13`.
   * HTLCErc20 EIP-712 domain version 6 behind the HTLCCoordinator, Arkade
   * VHTLC script version 1, Bitcoin HTLC script version 1, Lightning via
   * Boltz/Spark hold invoices. Clients still sending the legacy
   * `x-satora-server-version` header are treated as this epoch.
   */
  V1: 1,
  /**
   * EVM components only (`evm-erc20-htlc`, `evm-native-htlc`): HTLCErc20 v7
   * behind HTLCCoordinator v5, HTLCNative v2. The depositor is the lock's
   * `sender` and the Permit2 funding witness binds `refundAddress =
   * depositor`; collaborative refund and rehash. Servers keep creating swaps
   * for epoch 1 clients on the v6 deployment.
   */
  V2: 2,
} as const;

export type ProtocolEpoch = (typeof ProtocolEpoch)[keyof typeof ProtocolEpoch];

/** Protocol epoch this SDK speaks for each component. */
export const PROTOCOL_VERSIONS: Record<ProtocolComponent, ProtocolEpoch> = {
  [ProtocolComponent.BitcoinHtlc]: ProtocolEpoch.V1,
  [ProtocolComponent.ArkadeVhtlc]: ProtocolEpoch.V1,
  [ProtocolComponent.Lightning]: ProtocolEpoch.V1,
  [ProtocolComponent.EvmErc20Htlc]: ProtocolEpoch.V2,
  [ProtocolComponent.EvmNativeHtlc]: ProtocolEpoch.V2,
};

export function protocolHeaderName(component: ProtocolComponent): string {
  return `x-satora-${component}-version`;
}

/**
 * Every component header with the epoch this SDK speaks. Constant, so it is
 * attached to every API request rather than looked up per swap.
 */
export const PROTOCOL_HEADERS: Readonly<Record<string, string>> = Object.freeze(
  Object.fromEntries(
    Object.values(ProtocolComponent).map((component) => [
      protocolHeaderName(component),
      String(PROTOCOL_VERSIONS[component]),
    ]),
  ),
);
