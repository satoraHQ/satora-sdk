import createClient from "openapi-fetch";
import type { components, paths } from "../generated/api.js";
import { CLIENT_AGENT, SATORA_SERVER_VERSION } from "../version.js";

export type ApiClient = ReturnType<typeof createClient<paths>>;

export type { paths, components };

// Re-export commonly used types for convenience
export type TokenId = components["schemas"]["TokenId"];
export type TokenInfo = components["schemas"]["TokenInfo"];
export type TokenInfos = components["schemas"]["TokenInfos"];
export type SwapStatus = components["schemas"]["SwapStatus"];
export type ServiceStatus = components["schemas"]["ServiceStatus"];
export type StatusResponse = components["schemas"]["StatusResponse"];
export type GetSwapResponse = components["schemas"]["GetSwapResponse"];
export type BulkStatusRequest = components["schemas"]["BulkStatusRequest"];
export type BulkStatusResponse = components["schemas"]["BulkStatusResponse"];
export type SwapStatusEntry = components["schemas"]["SwapStatusEntry"];
export type BtcToArkadeSwapResponse =
  components["schemas"]["BtcToArkadeSwapResponse"];
export type EvmToArkadeSwapResponse =
  components["schemas"]["EvmToArkadeSwapResponse"];

// Gasless claim types
export type ClaimGaslessRequest = components["schemas"]["ClaimGaslessRequest"];
export type ClaimGaslessResponse =
  components["schemas"]["ClaimGaslessResponse"];
export type RedeemAndSwapResponse =
  components["schemas"]["RedeemAndSwapResponse"];

// Arkade-to-EVM (generic endpoint) types
export type ArkadeToEvmSwapRequest =
  components["schemas"]["ArkadeToEvmSwapRequest"];
export type ArkadeToEvmSwapResponse =
  components["schemas"]["ArkadeToEvmSwapResponse"];
export type DexCallData = components["schemas"]["DexCallData"];

// Bitcoin-to-EVM (generic endpoint) types
export type BitcoinToEvmSwapRequest =
  components["schemas"]["BitcoinToEvmSwapRequest"];
export type BitcoinToEvmSwapResponse =
  components["schemas"]["BitcoinToEvmSwapResponse"];

// EVM-to-Arkade (generic endpoint) types
export type EvmToArkadeGenericSwapRequest =
  components["schemas"]["EvmToArkadeGenericSwapRequest"];

// EVM-to-Lightning types
export type EvmToLightningSwapRequest =
  components["schemas"]["EvmToLightningSwapRequest"];
export type EvmToLightningSwapResponse =
  components["schemas"]["EvmToLightningSwapResponse"];

// EVM-to-Bitcoin (generic endpoint) types
export type EvmToBitcoinSwapRequest =
  components["schemas"]["EvmToBitcoinSwapRequest"];
export type EvmToBitcoinSwapResponse =
  components["schemas"]["EvmToBitcoinSwapResponse"];

// Lightning-to-Arkade types
export type LightningToArkadeSwapResponse =
  components["schemas"]["LightningToArkadeSwapResponse"];
export type LightningToEvmSwapResponse =
  components["schemas"]["LightningToEvmSwapResponse"];

// Arkade-to-Lightning types
export type ArkadeToLightningSwapResponse =
  components["schemas"]["ArkadeToLightningSwapResponse"];

// `SwapPairInfo`, `SwapPairsResponse`, `QuoteResponse`, `Chain`,
// `Token`, `TokenAmount`, `DexQuote*`, and `NetworkFee*` are now
// hand-written in `../types/`. Don't re-export them here — the SDK's
// public type surface is the hand-written module, not the OpenAPI
// codegen.

export interface ApiClientOptions {
  baseUrl: string;
  defaultHeaders?: Record<string, string>;
}

export function createApiClient(options: ApiClientOptions): ApiClient {
  const headers: Record<string, string> = {
    "X-Lendaswap-Client": CLIENT_AGENT,
    "x-satora-server-version": SATORA_SERVER_VERSION,
    ...(options.defaultHeaders ?? {}),
  };

  return createClient<paths>({
    baseUrl: options.baseUrl,
    headers,
  });
}
