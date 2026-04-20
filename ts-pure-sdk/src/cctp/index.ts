/**
 * CCTP (Cross-Chain Transfer Protocol) module.
 *
 * Provides utilities for bridging USDC cross-chain via Circle's CCTP:
 * - Domain ID mappings and contract addresses
 * - Attestation polling (IRIS API)
 * - Address conversion helpers
 */

export {
  type AttestationResult,
  type FetchAttestationOptions,
  fetchAttestation,
  type TrackCctpMessageOptions,
  trackCctpMessage,
} from "./attestation.js";
export {
  CCTP_DOMAINS,
  type CctpChainName,
  EURC_ADDRESSES,
  FINALITY_FAST,
  FINALITY_STANDARD,
  FORWARDING_FEE_ETHEREUM,
  FORWARDING_FEE_OTHER,
  FORWARDING_SERVICE_HOOK_DATA,
  IRIS_API_MAINNET,
  IRIS_API_TESTNET,
  MESSAGE_TRANSMITTER_ADDRESSES,
  MESSAGE_TRANSMITTER_V2,
  TOKEN_MESSENGER_ADDRESSES,
  TOKEN_MESSENGER_V2,
  USAT_ADDRESSES,
  USDC_ADDRESSES,
} from "./constants.js";
export {
  computeCctpFastFee,
  type FetchCctpFeeOptions,
  fetchCctpFee,
  getCachedCctpFee,
  type IrisFeeEntry,
  type IrisForwardFeeTiers,
} from "./fee.js";
export type {
  AttestationResponse,
  AttestationStatus,
  BridgeParams,
  BurnResult,
  CctpMessageResult,
  CctpMessageStatus,
  MintResult,
} from "./types.js";

export {
  addressToBytes32,
  bytes32ToAddress,
  getDomain,
  needsBridge,
} from "./utils.js";
