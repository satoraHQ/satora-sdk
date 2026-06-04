import { contractHandlers, isArkContract, networks } from "@arkade-os/sdk";
import { schnorr } from "@noble/curves/secp256k1.js";
import { hex } from "@scure/base";
import { afterEach, describe, expect, it } from "vitest";
import {
  decodeEscrowArkContract,
  encodeEscrowArkContract,
  escrowCreateContractParams,
} from "./ark-contract.js";
import { ESCROW_2OF2_CONTRACT_TYPE } from "./contract-handler.js";
import { type EscrowScriptOptions, EscrowVtxoScript } from "./escrow-script.js";

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

describe("escrow arkcontract handoff", () => {
  afterEach(() => {
    contractHandlers.unregister(ESCROW_2OF2_CONTRACT_TYPE);
  });

  it("encodes a recognizable arkcontract string", () => {
    const encoded = encodeEscrowArkContract(options);
    expect(isArkContract(encoded)).toBe(true);
    expect(encoded).toContain(`arkcontract=${ESCROW_2OF2_CONTRACT_TYPE}`);
    expect(encoded).toContain(
      `sellerPubKey=${hex.encode(options.sellerPubKey)}`,
    );
  });

  it("round-trips encode → decode to the same script and address", () => {
    const expected = new EscrowVtxoScript(options);
    const encoded = encodeEscrowArkContract(options);

    // decode must work even though the handler is not pre-registered.
    expect(contractHandlers.has(ESCROW_2OF2_CONTRACT_TYPE)).toBe(false);
    const contract = decodeEscrowArkContract(encoded, aspPubKey, network);

    expect(contract.type).toBe(ESCROW_2OF2_CONTRACT_TYPE);
    expect(contract.script).toBe(hex.encode(expected.pkScript));
    expect(contract.address).toBe(expected.arkAddress(network));
    expect(contract.params).toEqual({
      sellerPubKey: hex.encode(options.sellerPubKey),
      arbiterPubKey: hex.encode(options.arbiterPubKey),
      aspPubKey: hex.encode(aspPubKey),
      exitTimelock: contract.params.exitTimelock,
    });
  });

  it("escrowCreateContractParams matches the decoded script/address", () => {
    const params = escrowCreateContractParams(options, network, {
      label: "trade-42",
    });
    const expected = new EscrowVtxoScript(options);

    expect(params.type).toBe(ESCROW_2OF2_CONTRACT_TYPE);
    expect(params.script).toBe(hex.encode(expected.pkScript));
    expect(params.address).toBe(expected.arkAddress(network));
    expect(params.label).toBe("trade-42");
  });

  it("carries metadata through decode", () => {
    const encoded = encodeEscrowArkContract(options);
    const contract = decodeEscrowArkContract(encoded, aspPubKey, network, {
      label: "lightning-receive",
      metadata: { offerId: "abc" },
    });
    expect(contract.label).toBe("lightning-receive");
    expect(contract.metadata).toEqual({ offerId: "abc" });
  });
});
