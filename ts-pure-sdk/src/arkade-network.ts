import { type NetworkName, networks } from "@arkade-os/sdk";

/** Default Arkade server URL by network */
export const DEFAULT_ARKADE_URLS: Record<string, string> = {
  bitcoin: "https://arkade.computer",
  mainnet: "https://arkade.computer",
  signet: "https://mutinynet.arkade.sh",
  mutinynet: "https://mutinynet.arkade.sh",
};

export function getNetworkName(network: string): NetworkName {
  switch (network.toLowerCase()) {
    case "mainnet":
    case "bitcoin":
      return "bitcoin";
    case "testnet":
      return "testnet";
    case "signet":
      return "signet";
    case "mutinynet":
      return "mutinynet";
    case "regtest":
      return "regtest";
    default:
      throw new Error(`Unknown network: ${network}`);
  }
}

export function getNetworkHrp(networkName: NetworkName): string {
  return networks[networkName].hrp;
}

export function resolveArkadeServerUrl(
  network: string,
  arkadeServerUrl?: string,
): string {
  const networkName = getNetworkName(network);
  return resolveArkadeServerUrlByName(networkName, arkadeServerUrl);
}

export function resolveArkadeServerUrlByName(
  networkName: NetworkName,
  arkadeServerUrl?: string,
): string {
  const serverUrl = arkadeServerUrl ?? DEFAULT_ARKADE_URLS[networkName];
  if (!serverUrl) {
    throw new Error(
      `No Arkade server URL configured for network: ${networkName}`,
    );
  }
  return serverUrl;
}
