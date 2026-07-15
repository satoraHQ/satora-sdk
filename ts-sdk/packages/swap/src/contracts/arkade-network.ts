/**
 * Default Arkade server URLs by Bitcoin network, mirroring the legacy SDK's
 * `arkade-network.ts`. Used so tracking can auto-build the Arkade manager without
 * an explicit server URL; a caller can still override via
 * `ClientBuilder.withArkadeServerUrl`.
 */
const DEFAULT_ARKADE_URLS: Record<string, string> = {
  bitcoin: "https://arkade.computer",
  mainnet: "https://arkade.computer",
  signet: "https://mutinynet.arkade.sh",
  mutinynet: "https://mutinynet.arkade.sh",
};

/**
 * The default Ark server URL for a Bitcoin network name (case-insensitive), or
 * `undefined` for networks without a public default (e.g. regtest/testnet),
 * where a URL must be supplied explicitly.
 */
export function defaultArkadeServerUrl(network: string): string | undefined {
  return DEFAULT_ARKADE_URLS[network.toLowerCase()];
}
