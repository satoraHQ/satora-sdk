/**
 * Shared types for the CCTP-inbound flow.
 */

/**
 * Account-abstraction configuration for the CCTP-inbound path.
 *
 * The CCTP-inbound flow requires an ERC-4337 bundler and paymaster to
 * fund the `receiveMessage + approve + HTLC-create` UserOp atomically
 * on the settlement chain (Arbitrum). The user's connected wallet owns
 * the Kernel smart account; the SDK just wires the plumbing.
 *
 * All URLs point to the same Alchemy app (bundler and Gas Manager share
 * a base URL); the policy id is passed via ERC-7677 paymaster context.
 */
export interface AaConfig {
  /**
   * Bundler JSON-RPC URL (also serves as the paymaster URL for Alchemy
   * Gas Manager via ERC-7677 `pm_*` methods).
   *
   * @example `https://arb-mainnet.g.alchemy.com/v2/<API_KEY>`
   */
  bundlerUrl: string;

  /**
   * Alchemy Gas Manager policy id (UUID). Passed to the paymaster RPC
   * via the ERC-7677 context object so the bundler knows which policy
   * is sponsoring this UserOp.
   */
  paymasterPolicyId: string;
}
