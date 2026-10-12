/**
 * Wallet-agnostic Solana signer for the CCTP-inbound flow (USDC on Solana →
 * Arbitrum → BTC). Wrap any wallet with it: a Wallet Standard wallet
 * (`solana:signAndSendTransaction`), Phantom's provider, a raw keypair.
 */
export interface SolanaSigner {
  /** The wallet's public key (base58). Owns the USDC and pays the fees. */
  publicKey: string;
  /**
   * Add the wallet's signature to `serializedTransaction` (a legacy
   * wire-format transaction the SDK built and already partially signed with
   * a generated event-data keypair), broadcast it, and return the
   * transaction signature (base58).
   *
   * The wallet must not rewrite the message (e.g. to add its own priority
   * fee): that invalidates the SDK's signature. The SDK sets the compute
   * budget itself.
   */
  signAndSendTransaction(serializedTransaction: Uint8Array): Promise<string>;
}
