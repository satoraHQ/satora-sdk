import {
  Client as LegacyClient,
  type StoredSwap,
} from "@lendasat/lendaswap-sdk-pure";
import { describe, expect, it } from "vitest";
import type { HtlcObservation } from "./actions/types.js";
import { Client } from "./client.js";
import {
  type ContractManager,
  type HtlcRef,
  htlcKey,
  type Ledger,
} from "./contracts/types.js";

/** A monitor that only records what it was asked to register. */
class FakeManager implements ContractManager {
  readonly ledger: Ledger;
  readonly registered = new Set<string>();
  constructor(ledger: Ledger) {
    this.ledger = ledger;
  }
  register = async (ref: HtlcRef): Promise<void> => {
    this.registered.add(htlcKey(ref));
  };
  unregister = async (ref: HtlcRef): Promise<void> => {
    this.registered.delete(htlcKey(ref));
  };
  getState = (_ref: HtlcRef): HtlcObservation | undefined => undefined;
  chainNow = (_ref: HtlcRef): number | undefined => undefined;
  onEvent = (): (() => void) => () => {};
  refresh = async (): Promise<void> => {};
  dispose = (): void => {};
}

/** A legacy-client stand-in that satisfies `instanceof` but returns canned swaps. */
function fakeLegacy(swaps: StoredSwap[]): LegacyClient {
  const legacy = Object.create(LegacyClient.prototype) as LegacyClient;
  Object.assign(legacy, { listAllSwaps: async () => swaps });
  return legacy;
}

function managers() {
  const arkade = new FakeManager("arkade");
  const evm = new FakeManager("evm");
  return {
    arkade,
    evm,
    map: new Map<Ledger, ContractManager>([
      ["arkade", arkade],
      ["evm", evm],
    ]),
  };
}

// A valid arkade_to_evm swap (BIP340 pubkeys; delays are multiples of 512).
const arkadeEvmSwap = {
  response: {
    direction: "arkade_to_evm",
    id: "swap-1",
    sender_pk:
      "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9",
    receiver_pk:
      "dff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659",
    arkade_server_pk:
      "dd308afec5777e13121fa72b9cc1b7cc0139715309b086c960e18fd969774eb8",
    hash_lock:
      "abababababababababababababababababababababababababababababababab",
    btc_vhtlc_address: "ark1qexample",
    vhtlc_refund_locktime: 1_000_000,
    evm_refund_locktime: 900_000,
    evm_chain_id: 137,
    evm_htlc_address: "0xhtlc",
    evm_expected_sats: "1000",
    client_evm_address: "0xclient",
    server_evm_address: "0xserver",
    wbtc_address: "0xwbtc",
    source_amount: "1000",
    unilateral_claim_delay: 512,
    unilateral_refund_delay: 1024,
    unilateral_refund_without_receiver_delay: 1536,
  },
} as unknown as StoredSwap;

const unsupportedSwap = {
  response: { direction: "future_direction", id: "swap-2" },
} as unknown as StoredSwap;

/** Tracking config with an explicit managers override (skips auto-construction). */
const withManagers = (map: Map<Ledger, ContractManager>) => ({
  enabled: true,
  managers: map,
  refreshIntervalMs: 0, // no timer in tests
});

describe("Client tracking", () => {
  it("rejects startTracking when tracking is disabled", () => {
    const client = new Client(fakeLegacy([]), { enabled: false });
    return expect(client.startTracking()).rejects.toThrow(/disabled/);
  });

  it("requires startTracking before subscribeToActions", () => {
    const m = managers();
    const client = new Client(fakeLegacy([]), withManagers(m.map));
    expect(() => client.subscribeToActions(() => {})).toThrow(/startTracking/);
  });

  it("registers both legs of a supported swap", async () => {
    const m = managers();
    const client = new Client(fakeLegacy([arkadeEvmSwap]), withManagers(m.map));
    await client.startTracking();
    expect(m.arkade.registered.size).toBe(1);
    expect(m.evm.registered.size).toBe(1);
  });

  it("skips swaps whose ledgers aren't observable yet", async () => {
    const m = managers();
    const client = new Client(
      fakeLegacy([unsupportedSwap]),
      withManagers(m.map),
    );
    await client.startTracking();
    expect(m.arkade.registered.size).toBe(0);
    expect(m.evm.registered.size).toBe(0);
  });

  it("is idempotent — a second startTracking doesn't re-register", async () => {
    const m = managers();
    const client = new Client(fakeLegacy([arkadeEvmSwap]), withManagers(m.map));
    await client.startTracking();
    await client.startTracking();
    expect(m.arkade.registered.size).toBe(1);
  });

  it("subscribeToActions returns an unsubscribe and stopTracking is safe", async () => {
    const m = managers();
    const client = new Client(fakeLegacy([]), withManagers(m.map));
    await client.startTracking();
    const unsub = client.subscribeToActions(() => {});
    expect(typeof unsub).toBe("function");
    unsub();
    expect(() => client.stopTracking()).not.toThrow();
  });

  it("unregisters every tracked leg on stopTracking (no leaked manager watches)", async () => {
    const m = managers();
    const client = new Client(fakeLegacy([arkadeEvmSwap]), withManagers(m.map));
    await client.startTracking();
    expect(m.arkade.registered.size).toBe(1);
    expect(m.evm.registered.size).toBe(1);
    client.stopTracking();
    // Both legs released, so the managers stop watching this swap.
    expect(m.arkade.registered.size).toBe(0);
    expect(m.evm.registered.size).toBe(0);
  });
});
