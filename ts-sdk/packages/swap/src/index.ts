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
// Satora-native features.
export { deriveSwapActions } from "./actions/derive.js";
export { deriveSwapStatus } from "./actions/status.js";
export type * from "./actions/types.js";
export { Client, ClientBuilder } from "./client.js";
// Observe-mode tracking: per-ledger monitors + the recovery-bundle mapper.
export {
  ArkadeContractManager,
  type ArkadeContractManagerDeps,
  type ArkadeCreateConfig,
} from "./contracts/arkade-manager.js";
export { defaultArkadeServerUrl } from "./contracts/arkade-network.js";
export {
  type ArkadeVhtlcInput,
  buildArkadeVhtlcRef,
} from "./contracts/arkade-vhtlc.js";
export {
  type BitcoinChainReader,
  BitcoinContractManager,
  type BitcoinContractManagerDeps,
  type BitcoinCreateConfig,
} from "./contracts/bitcoin-manager.js";
export {
  DEFAULT_ESPLORA_URLS,
  esploraReader,
  htlcFactsFromEsploraTxs,
} from "./contracts/bitcoin-reader-esplora.js";
export {
  type EvmChainReader,
  EvmContractManager,
  type EvmContractManagerDeps,
} from "./contracts/evm-manager.js";
export {
  createEvmRpcReader,
  DEFAULT_EVM_RPCS,
  defaultEvmReaders,
  type EvmLogClient,
  evmReaderFromClient,
} from "./contracts/evm-reader-viem.js";
export type * from "./contracts/types.js";
export { htlcKey } from "./contracts/types.js";
export { swapToTracked } from "./tracker/from-swap.js";
export {
  type ActionSubscriber,
  SwapTracker,
  type TrackedSwap,
} from "./tracker/swap-tracker.js";
