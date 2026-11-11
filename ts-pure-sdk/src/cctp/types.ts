/**
 * CCTP type definitions.
 */

/** Status of a CCTP attestation request. */
export type AttestationStatus = "pending_confirmations" | "complete";

/** Status of a CCTP cross-chain message tracked via IRIS. */
export type CctpMessageStatus =
  | "PENDING"
  | "CONFIRMING"
  | "FORWARDING"
  | "COMPLETE"
  | "FAILED";

/** Response from the IRIS attestation API (V2). */
export interface AttestationResponse {
  messages: Array<{
    message: string;
    attestation: string;
    status: AttestationStatus;
    eventNonce: string;
    /** Forwarding state (present when CCTP forwarding service is used). */
    forwardState?: string | null;
    /** Transaction hash of the forward/mint on the destination chain. */
    forwardTxHash?: string | null;
    /**
     * Transaction that minted on the destination chain, whoever sent it.
     * Set for messages delivered without the forwarding service too.
     */
    destinationMintTxHash?: string | null;
    /** Decoded CCTP message with amounts and fees. */
    decodedMessage?: {
      sourceDomain: string;
      destinationDomain: string;
      decodedMessageBody?: {
        /** Amount received in smallest units (after fee deduction). */
        amount: string;
        /** Mint recipient address on destination chain. */
        mintRecipient: string;
        /** Maximum fee allowed. */
        maxFee: string;
        /** Fee actually executed in smallest units. */
        feeExecuted: string;
      };
    } | null;
  }>;
}

/** Result of tracking a CCTP cross-chain message. */
export interface CctpMessageResult {
  /** Current message status. */
  status: CctpMessageStatus;
  /**
   * Transaction that delivered the USDC on the destination chain: Circle's
   * forward, or the mint when the message was delivered without the
   * forwarding service.
   */
  forwardTxHash?: string;
  /** Amount received on destination in smallest units (after fee deduction). */
  amount?: string;
  /** CCTP fee executed in smallest units. */
  feeExecuted?: string;
}

/** Parameters for initiating a CCTP bridge. */
export interface BridgeParams {
  /** Amount of USDC to bridge (in smallest unit, e.g. 1000000 = 1 USDC). */
  amount: bigint;
  /** Source chain name (e.g. "Polygon"). */
  sourceChain: string;
  /** Destination chain name (e.g. "Ethereum"). */
  destinationChain: string;
  /** Recipient address on the destination chain. */
  mintRecipient: string;
}

/** Result of a CCTP burn (source chain). */
export interface BurnResult {
  /** Transaction hash of the burn. */
  txHash: string;
  /** CCTP message nonce. */
  nonce: bigint;
  /** Raw message bytes (hex-encoded). */
  messageBytes: string;
  /** keccak256 hash of the message bytes. */
  messageHash: string;
  /** Source domain ID. */
  sourceDomain: number;
}

/** Result of a CCTP mint (destination chain). */
export interface MintResult {
  /** Transaction hash of the mint on the destination chain. */
  txHash: string;
}
