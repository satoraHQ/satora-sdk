import {
  type Contract,
  contractHandlers,
  networks,
  type PathContext,
  timelockToSequence,
  type VirtualCoin,
} from "@arkade-os/sdk";
import { schnorr } from "@noble/curves/secp256k1.js";
import { hex } from "@scure/base";
import { afterEach, describe, expect, it } from "vitest";
import {
  ESCROW_2OF2_CONTRACT_TYPE,
  EscrowContractHandler,
  registerEscrowContractHandler,
} from "./contract-handler.js";
import { type EscrowScriptOptions, EscrowVtxoScript } from "./escrow-script.js";

/** Deterministic valid x-only pubkey from a small seed. */
function xOnlyPubKey(seed: number): Uint8Array {
  const sk = new Uint8Array(32);
  sk[31] = seed;
  return schnorr.getPublicKey(sk);
}

const sellerPubKey = xOnlyPubKey(1);
const arbiterPubKey = xOnlyPubKey(2);
const aspPubKey = xOnlyPubKey(3);
const exitTimelock = { type: "blocks", value: 4320n } as const;

const options: EscrowScriptOptions = {
  sellerPubKey,
  arbiterPubKey,
  aspPubKey,
  exitTimelock,
};

const escapeSeq = timelockToSequence(exitTimelock);

function makeContract(): Contract {
  const script = new EscrowVtxoScript(options);
  return {
    type: ESCROW_2OF2_CONTRACT_TYPE,
    params: EscrowContractHandler.serializeParams(options),
    script: hex.encode(script.pkScript),
    address: script.arkAddress(networks.regtest),
    state: "active",
    createdAt: 0,
  };
}

/** Minimal VirtualCoin sufficient for CSV evaluation. */
function fakeVtxo(blockHeight?: number): VirtualCoin {
  return {
    txid: "00".repeat(32),
    vout: 0,
    value: 100_000,
    status: { confirmed: blockHeight !== undefined, block_height: blockHeight },
    createdAt: new Date(0),
    script: "",
    isUnrolled: false,
    virtualStatus: { state: "settled" },
  } as unknown as VirtualCoin;
}

describe("EscrowContractHandler", () => {
  afterEach(() => {
    contractHandlers.unregister(ESCROW_2OF2_CONTRACT_TYPE);
  });

  it("round-trips params through serialize/deserialize", () => {
    const wire = EscrowContractHandler.serializeParams(options);
    expect(wire).toEqual({
      sellerPubKey: hex.encode(sellerPubKey),
      arbiterPubKey: hex.encode(arbiterPubKey),
      aspPubKey: hex.encode(aspPubKey),
      exitTimelock: escapeSeq.toString(),
    });

    const back = EscrowContractHandler.deserializeParams(wire);
    expect(back.sellerPubKey).toEqual(sellerPubKey);
    expect(back.arbiterPubKey).toEqual(arbiterPubKey);
    expect(back.aspPubKey).toEqual(aspPubKey);
    expect(back.exitTimelock).toEqual({ type: "blocks", value: 4320n });
  });

  it("createScript reproduces the same pkScript as a direct construction", () => {
    const contract = makeContract();
    const built = EscrowContractHandler.createScript(contract.params);
    expect(hex.encode(built.pkScript)).toBe(contract.script);
  });

  it("exposes SDK-conformant forfeit()/exit() aliases", () => {
    const script = new EscrowVtxoScript(options);
    // Required by the SDK's deriveContractTapscripts during vtxo annotation.
    expect(script.forfeit()).toEqual(script.cooperativeLeaf());
    expect(script.exit()).toEqual(script.escapeLeaf());
  });

  it("registers idempotently into the global handler registry", () => {
    expect(contractHandlers.has(ESCROW_2OF2_CONTRACT_TYPE)).toBe(false);
    registerEscrowContractHandler();
    registerEscrowContractHandler(); // second call must not throw
    expect(contractHandlers.has(ESCROW_2OF2_CONTRACT_TYPE)).toBe(true);
    expect(contractHandlers.get(ESCROW_2OF2_CONTRACT_TYPE)).toBe(
      EscrowContractHandler,
    );
  });

  it("selectPath returns the cooperative leaf when collaborative (any role)", () => {
    const script = new EscrowVtxoScript(options);
    const contract = makeContract();
    const ctx: PathContext = { collaborative: true, currentTime: 0 };

    const sel = EscrowContractHandler.selectPath(script, contract, ctx);
    expect(sel).not.toBeNull();
    expect(sel?.leaf).toEqual(script.cooperativeLeaf());
    expect(sel?.sequence).toBeUndefined();
  });

  it("selectPath returns null for the seller once collaboration is unavailable", () => {
    const script = new EscrowVtxoScript(options);
    const contract = makeContract();
    const ctx: PathContext = {
      collaborative: false,
      currentTime: 0,
      walletPubKey: hex.encode(sellerPubKey),
      vtxo: fakeVtxo(100),
      blockHeight: 1_000_000,
    };

    expect(EscrowContractHandler.selectPath(script, contract, ctx)).toBeNull();
  });

  it("selectPath returns the escape leaf for the arbiter after the CSV", () => {
    const script = new EscrowVtxoScript(options);
    const contract = makeContract();
    const ctx: PathContext = {
      collaborative: false,
      currentTime: 0,
      role: "arbiter",
      vtxo: fakeVtxo(100),
      blockHeight: 100 + 4320,
    };

    const sel = EscrowContractHandler.selectPath(script, contract, ctx);
    expect(sel).not.toBeNull();
    expect(sel?.leaf).toEqual(script.escapeLeaf());
    expect(sel?.sequence).toBe(escapeSeq);
  });

  it("selectPath returns null for the arbiter before the CSV elapses", () => {
    const script = new EscrowVtxoScript(options);
    const contract = makeContract();
    const ctx: PathContext = {
      collaborative: false,
      currentTime: 0,
      role: "arbiter",
      vtxo: fakeVtxo(100),
      blockHeight: 100 + 4319,
    };

    expect(EscrowContractHandler.selectPath(script, contract, ctx)).toBeNull();
  });
});
