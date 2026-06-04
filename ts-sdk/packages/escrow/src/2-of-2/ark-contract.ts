import {
  type Contract,
  type ContractState,
  type CreateContractParams,
  contractFromArkContractWithAddress,
  encodeArkContract,
  type Network,
} from "@arkade-os/sdk";
import { hex } from "@scure/base";
import {
  ESCROW_2OF2_CONTRACT_TYPE,
  EscrowContractHandler,
  registerEscrowContractHandler,
} from "./contract-handler.js";
import { type EscrowScriptOptions, EscrowVtxoScript } from "./escrow-script.js";

/** Optional descriptive fields carried alongside a registered contract. */
export interface EscrowContractMeta {
  label?: string;
  state?: ContractState;
  metadata?: Record<string, unknown>;
}

/**
 * Build {@link CreateContractParams} for a 2-of-2 escrow: derives the pkScript
 * and funding address from the escrow parameters, ready to hand to
 * `ContractManager.createContract`.
 */
export function escrowCreateContractParams(
  options: EscrowScriptOptions,
  network: Network,
  meta: EscrowContractMeta = {},
): CreateContractParams {
  const script = new EscrowVtxoScript(options);
  return {
    label: meta.label,
    type: ESCROW_2OF2_CONTRACT_TYPE,
    params: EscrowContractHandler.serializeParams(options),
    script: hex.encode(script.pkScript),
    address: script.arkAddress(network),
    state: meta.state,
    metadata: meta.metadata,
  };
}

/**
 * Encode a 2-of-2 escrow as a NArk-compatible `arkcontract=` string for the
 * server→client handoff. Carries the escrow parameters only — the receiver
 * re-derives the script and address via {@link decodeEscrowArkContract}.
 */
export function encodeEscrowArkContract(options: EscrowScriptOptions): string {
  // `encodeArkContract` reads only `type` and `params`.
  return encodeArkContract({
    type: ESCROW_2OF2_CONTRACT_TYPE,
    params: EscrowContractHandler.serializeParams(options),
  } as Contract);
}

/**
 * Decode an escrow `arkcontract=` string into a full {@link Contract},
 * re-deriving the pkScript and funding address from the embedded parameters.
 *
 * Registers the escrow handler if needed, so the caller does not have to.
 * `aspPubKey` is the ASP x-only key the funding address is built from.
 */
export function decodeEscrowArkContract(
  encoded: string,
  aspPubKey: Uint8Array,
  network: Network,
  meta: EscrowContractMeta = {},
): Contract {
  registerEscrowContractHandler();
  return contractFromArkContractWithAddress(encoded, aspPubKey, network.hrp, {
    label: meta.label,
    state: meta.state,
    metadata: meta.metadata,
  });
}
