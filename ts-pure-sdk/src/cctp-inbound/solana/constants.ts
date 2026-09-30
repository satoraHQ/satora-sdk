/**
 * Solana mainnet CCTPv2 program ids and the USDC mint, from Circle's
 * `solana-cctp-contracts` (`programs/v2`) and its published IDLs.
 */

import { USDC_ADDRESSES } from "../../cctp/constants.js";

/** CCTPv2 MessageTransmitterV2 program. */
export const SOLANA_MESSAGE_TRANSMITTER_V2 =
  "CCTPV2Sm4AdWt5296sk4P66VBZ7bEhcARwFaaS9YPbeC";
/** CCTPv2 TokenMessengerMinterV2 program. */
export const SOLANA_TOKEN_MESSENGER_MINTER_V2 =
  "CCTPV2vPZJS2u2BBsUoscuikbYjnpFmbFsvVuJdgUMQe";
/** Native USDC mint on Solana mainnet. */
export const SOLANA_USDC_MINT: string = USDC_ADDRESSES.Solana;
