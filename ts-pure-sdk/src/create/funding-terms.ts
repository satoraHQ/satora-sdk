import type { EvmFundingTerms } from "../evm/funding-pins.js";

/**
 * The funding terms of an EVM-sourced swap, read from its creation
 * response: the coordinator the server named then is the only Permit2
 * spender this client will sign for, and the HTLC, claimer, timelock,
 * tokens and input are what every later funding payload must match.
 */
export function fundingTermsOf(data: {
  evm_chain_id?: number | null;
  evm_coordinator_address?: string | null;
  evm_htlc_address?: string | null;
  evm_htlc_kind?: string | null;
  evm_htlc_version?: number | null;
  server_evm_address?: string | null;
  evm_refund_locktime?: number | null;
  source_amount?: string | number | null;
  source_token?: { token_id?: string | null } | null;
  evm_htlc_token?: string | null;
}): EvmFundingTerms | undefined {
  // A response without them records nothing: funding then fails closed.
  if (
    !data.evm_chain_id ||
    !data.evm_coordinator_address ||
    !data.evm_htlc_address ||
    !data.server_evm_address ||
    !data.evm_refund_locktime ||
    data.source_amount === undefined ||
    data.source_amount === null ||
    !data.source_token?.token_id
  ) {
    return undefined;
  }
  return {
    chainId: data.evm_chain_id,
    coordinator: data.evm_coordinator_address.toLowerCase(),
    htlcAddress: data.evm_htlc_address.toLowerCase(),
    htlcKind: data.evm_htlc_kind ?? undefined,
    htlcVersion: data.evm_htlc_version ?? undefined,
    claimAddress: data.server_evm_address.toLowerCase(),
    timelock: data.evm_refund_locktime,
    sourceToken: data.source_token.token_id.toLowerCase(),
    sourceAmount: String(data.source_amount),
    lockToken: data.evm_htlc_token?.toLowerCase(),
  };
}
