import {
  type Contract,
  type ContractEvent,
  type ContractVtxo,
  type ContractWithVtxos,
  type CreateContractParams,
  contractHandlers,
  networks,
} from "@arkade-os/sdk";
import { schnorr } from "@noble/curves/secp256k1.js";
import { hex } from "@scure/base";
import { afterEach, describe, expect, it, vi } from "vitest";
import { encodeEscrowArkContract } from "./ark-contract.js";
import { ESCROW_2OF2_CONTRACT_TYPE } from "./contract-handler.js";
import { type EscrowScriptOptions, EscrowVtxoScript } from "./escrow-script.js";
import { type EscrowManagerLike, EscrowMonitor } from "./monitor.js";

function xOnlyPubKey(seed: number): Uint8Array {
  const sk = new Uint8Array(32);
  sk[31] = seed;
  return schnorr.getPublicKey(sk);
}

const aspPubKey = xOnlyPubKey(3);
const network = networks.regtest;
const options: EscrowScriptOptions = {
  sellerPubKey: xOnlyPubKey(1),
  arbiterPubKey: xOnlyPubKey(2),
  aspPubKey,
  exitTimelock: { type: "blocks", value: 4320n },
};

/** In-memory manager double exposing an `emit` hook to drive events. */
class FakeManager implements EscrowManagerLike {
  readonly created: CreateContractParams[] = [];
  readonly deleted: string[] = [];
  disposed = false;
  private callbacks: Array<(e: ContractEvent) => void> = [];
  private store: Contract[] = [];

  createContract(params: CreateContractParams): Promise<Contract> {
    this.created.push(params);
    const contract: Contract = {
      ...params,
      state: params.state ?? "active",
      createdAt: 0,
    };
    this.store.push(contract);
    return Promise.resolve(contract);
  }

  getContractsWithVtxos(): Promise<ContractWithVtxos[]> {
    return Promise.resolve(
      this.store.map((contract) => ({ contract, vtxos: [] })),
    );
  }

  onContractEvent(cb: (e: ContractEvent) => void): () => void {
    this.callbacks.push(cb);
    return () => {
      this.callbacks = this.callbacks.filter((c) => c !== cb);
    };
  }

  deleteContract(script: string): Promise<void> {
    this.deleted.push(script);
    return Promise.resolve();
  }

  dispose(): void {
    this.disposed = true;
  }

  emit(event: ContractEvent): void {
    for (const cb of this.callbacks) cb(event);
  }
}

function vtxo(value: number): ContractVtxo {
  return { value } as unknown as ContractVtxo;
}

function escrowContract(): Contract {
  const script = new EscrowVtxoScript(options);
  return {
    type: ESCROW_2OF2_CONTRACT_TYPE,
    params: {},
    script: hex.encode(script.pkScript),
    address: script.arkAddress(network),
    state: "active",
    createdAt: 0,
  };
}

describe("EscrowMonitor", () => {
  afterEach(() => {
    contractHandlers.unregister(ESCROW_2OF2_CONTRACT_TYPE);
  });

  it("registers the handler and watches an escrow by params", async () => {
    const manager = new FakeManager();
    const monitor = EscrowMonitor.fromManager(manager);
    expect(contractHandlers.has(ESCROW_2OF2_CONTRACT_TYPE)).toBe(true);

    const contract = await monitor.watch(options, network, { label: "t1" });

    expect(manager.created).toHaveLength(1);
    expect(contract.type).toBe(ESCROW_2OF2_CONTRACT_TYPE);
    expect(contract.script).toBe(
      hex.encode(new EscrowVtxoScript(options).pkScript),
    );
    expect(contract.label).toBe("t1");
  });

  it("watches an escrow received as an arkcontract string", async () => {
    const manager = new FakeManager();
    const monitor = EscrowMonitor.fromManager(manager);

    const encoded = encodeEscrowArkContract(options);
    const contract = await monitor.watchArkContract(
      encoded,
      aspPubKey,
      network,
    );

    expect(contract.script).toBe(
      hex.encode(new EscrowVtxoScript(options).pkScript),
    );
    // createContract must receive params without `createdAt`.
    expect(manager.created[0]).not.toHaveProperty("createdAt");
  });

  it("fires onFunded for vtxo_received on an escrow, with the total", () => {
    const manager = new FakeManager();
    const monitor = EscrowMonitor.fromManager(manager);
    const onFunded = vi.fn();
    monitor.onFunded(onFunded);

    manager.emit({
      type: "vtxo_received",
      contractScript: "deadbeef",
      vtxos: [vtxo(60_000), vtxo(40_000)],
      contract: escrowContract(),
      timestamp: 0,
    });

    expect(onFunded).toHaveBeenCalledTimes(1);
    expect(onFunded.mock.calls[0][0].totalSats).toBe(100_000);
  });

  it("fires onReleased for vtxo_spent on an escrow", () => {
    const manager = new FakeManager();
    const monitor = EscrowMonitor.fromManager(manager);
    const onReleased = vi.fn();
    monitor.onReleased(onReleased);

    manager.emit({
      type: "vtxo_spent",
      contractScript: "deadbeef",
      vtxos: [vtxo(100_000)],
      contract: escrowContract(),
      timestamp: 0,
    });

    expect(onReleased).toHaveBeenCalledTimes(1);
  });

  it("ignores events for non-escrow contracts and after unsubscribe", () => {
    const manager = new FakeManager();
    const monitor = EscrowMonitor.fromManager(manager);
    const onFunded = vi.fn();
    const unsubscribe = monitor.onFunded(onFunded);

    // Non-escrow contract: ignored.
    manager.emit({
      type: "vtxo_received",
      contractScript: "x",
      vtxos: [vtxo(1)],
      contract: { ...escrowContract(), type: "default" },
      timestamp: 0,
    });
    expect(onFunded).not.toHaveBeenCalled();

    unsubscribe();
    manager.emit({
      type: "vtxo_received",
      contractScript: "x",
      vtxos: [vtxo(1)],
      contract: escrowContract(),
      timestamp: 0,
    });
    expect(onFunded).not.toHaveBeenCalled();
  });

  it("delegates unwatch and dispose to the manager", async () => {
    const manager = new FakeManager();
    const monitor = EscrowMonitor.fromManager(manager);

    await monitor.unwatch("scripthex");
    expect(manager.deleted).toEqual(["scripthex"]);

    monitor.dispose();
    expect(manager.disposed).toBe(true);
  });
});
