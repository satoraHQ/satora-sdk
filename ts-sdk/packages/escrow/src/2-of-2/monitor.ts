import {
  type Contract,
  ContractManager,
  type ContractManagerConfig,
  type ContractVtxo,
  type IContractManager,
  type Network,
} from "@arkade-os/sdk";
import {
  decodeEscrowArkContract,
  type EscrowContractMeta,
  escrowCreateContractParams,
} from "./ark-contract.js";
import {
  ESCROW_2OF2_CONTRACT_TYPE,
  registerEscrowContractHandler,
} from "./contract-handler.js";
import type { EscrowScriptOptions } from "./escrow-script.js";

/** The subset of `IContractManager` the monitor relies on. */
export type EscrowManagerLike = Pick<
  IContractManager,
  | "createContract"
  | "getContractsWithVtxos"
  | "onContractEvent"
  | "deleteContract"
  | "dispose"
>;

/** Funding observed at an escrow address. */
export interface EscrowFundedEvent {
  contract: Contract;
  vtxos: ContractVtxo[];
  /** Sum of the received vtxo values, in sats. */
  totalSats: number;
}

/** The escrow VTXO was spent (cooperative release or arbiter escape). */
export interface EscrowReleasedEvent {
  contract: Contract;
  vtxos: ContractVtxo[];
}

function totalSats(vtxos: ContractVtxo[]): number {
  return vtxos.reduce((acc, v) => acc + v.value, 0);
}

/**
 * A thin facade over the Arkade {@link ContractManager} scoped to 2-of-2
 * escrows. It registers the escrow handler, owns the manager lifecycle, and
 * exposes escrow-shaped helpers so a consumer never touches the handler
 * registry, the watcher, or the repositories directly.
 *
 * Watching, persistence, reconnection, and failsafe polling are all handled
 * by the underlying manager.
 */
export class EscrowMonitor {
  private constructor(private readonly manager: EscrowManagerLike) {}

  /**
   * Construct a monitor, creating and starting a real {@link ContractManager}
   * from the given providers and repositories.
   */
  static async create(config: ContractManagerConfig): Promise<EscrowMonitor> {
    registerEscrowContractHandler();
    const manager = await ContractManager.create(config);
    return new EscrowMonitor(manager);
  }

  /**
   * Wrap an existing manager (or a test double). Registers the escrow handler.
   */
  static fromManager(manager: EscrowManagerLike): EscrowMonitor {
    registerEscrowContractHandler();
    return new EscrowMonitor(manager);
  }

  /**
   * Register an escrow (built from its parameters) for monitoring. Returns the
   * persisted {@link Contract}; its `address` is where the seller funds.
   */
  watch(
    options: EscrowScriptOptions,
    network: Network,
    meta?: EscrowContractMeta,
  ): Promise<Contract> {
    return this.manager.createContract(
      escrowCreateContractParams(options, network, meta),
    );
  }

  /**
   * Register an escrow received as an `arkcontract=` string (the server→client
   * handoff) for monitoring.
   */
  watchArkContract(
    encoded: string,
    aspPubKey: Uint8Array,
    network: Network,
    meta?: EscrowContractMeta,
  ): Promise<Contract> {
    const { createdAt: _createdAt, ...params } = decodeEscrowArkContract(
      encoded,
      aspPubKey,
      network,
      meta,
    );
    return this.manager.createContract(params);
  }

  /** Stop monitoring an escrow by its pkScript hex. */
  unwatch(script: string): Promise<void> {
    return this.manager.deleteContract(script);
  }

  /** List all monitored escrows together with their current virtual outputs. */
  async listEscrows(): Promise<EscrowFundedEvent[]> {
    const all = await this.manager.getContractsWithVtxos({
      type: ESCROW_2OF2_CONTRACT_TYPE,
    });
    return all.map(({ contract, vtxos }) => ({
      contract,
      vtxos,
      totalSats: totalSats(vtxos),
    }));
  }

  /**
   * Fire `cb` when funds land at an escrow address (the FUNDED signal). Only
   * fires for escrow contracts. Returns an unsubscribe function.
   */
  onFunded(cb: (event: EscrowFundedEvent) => void): () => void {
    return this.manager.onContractEvent((event) => {
      if (
        event.type === "vtxo_received" &&
        event.contract.type === ESCROW_2OF2_CONTRACT_TYPE
      ) {
        cb({
          contract: event.contract,
          vtxos: event.vtxos,
          totalSats: totalSats(event.vtxos),
        });
      }
    });
  }

  /**
   * Fire `cb` when an escrow VTXO is spent (the RELEASED signal — cooperative
   * release or arbiter escape). Only fires for escrow contracts. Returns an
   * unsubscribe function.
   */
  onReleased(cb: (event: EscrowReleasedEvent) => void): () => void {
    return this.manager.onContractEvent((event) => {
      if (
        event.type === "vtxo_spent" &&
        event.contract.type === ESCROW_2OF2_CONTRACT_TYPE
      ) {
        cb({ contract: event.contract, vtxos: event.vtxos });
      }
    });
  }

  /** Release the manager's resources (stop watching, clear listeners). */
  dispose(): void {
    this.manager.dispose();
  }
}
