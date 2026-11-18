/**
 * Shared types for the CCTP-inbound flow.
 */

/**
 * Account-abstraction configuration for the CCTP-inbound path.
 *
 * The CCTP-inbound flow requires an ERC-4337 bundler to submit the
 * `receiveMessage + approve + HTLC-create` UserOp atomically on the
 * settlement chain (Arbitrum). The user's connected wallet owns the Kernel
 * smart account; the SDK just wires the plumbing.
 *
 * Hosted providers often expose normal chain RPC, bundler RPC, and paymaster
 * RPC on the same URL. Local E2E setups usually do not: Anvil serves normal
 * chain RPC while alto serves only ERC-4337 bundler methods. `rpcUrl` lets
 * callers split those endpoints.
 */
export interface AaConfig {
  /**
   * Bundler JSON-RPC URL.
   *
   * @example `https://arb-mainnet.g.alchemy.com/v2/<API_KEY>`
   * @example `http://localhost:4337` for local alto
   */
  bundlerUrl: string;

  /**
   * Optional normal EVM JSON-RPC URL. Defaults to `bundlerUrl` for hosted
   * providers that expose both normal chain RPC and bundler RPC on one URL.
   *
   * @example `http://localhost:8546` for local Arbitrum Anvil
   */
  rpcUrl?: string;

  /**
   * Optional Alchemy Gas Manager policy id (UUID). When present, the SDK wires
   * an ERC-7677 paymaster client and asks the paymaster to sponsor the UserOp.
   * When omitted, the UserOp is sent without a paymaster and the smart account
   * must have enough native gas token to pay for execution.
   */
  paymasterPolicyId?: string;

  /** Explicit local-test opt-in: fundSwapGasless may use account-paid gas.
   * The deposit account must be prefunded with native ETH. Never advertised
   * as sponsored; production deposit UIs should reject this mode.
   */
  allowSelfPaidFunding?: boolean;
}
