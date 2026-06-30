import { encodeAbiParameters, keccak256 as viemKeccak256 } from "viem";
import { describe, expect, it } from "vitest";
import {
  type CoordinatorCall,
  computeCoordinatorCallsHash,
} from "../src/evm/coordinator.js";

/**
 * The SDK computes `callsHash = keccak256(abi.encode(Call[]))` itself when it
 * publishes `redeemAndExecute` (instead of the server). It MUST match Solidity
 * `abi.encode(Call[])` byte-for-byte or `redeemBySig` reverts. We cross-check
 * our hand-rolled encoder against viem's canonical ABI encoder.
 */
describe("computeCoordinatorCallsHash", () => {
  const reference = (calls: CoordinatorCall[]): string =>
    viemKeccak256(
      encodeAbiParameters(
        [
          {
            type: "tuple[]",
            components: [
              { name: "target", type: "address" },
              { name: "value", type: "uint256" },
              { name: "data", type: "bytes" },
            ],
          },
        ],
        [
          calls.map((c) => ({
            target: c.target as `0x${string}`,
            value: c.value,
            data: c.data as `0x${string}`,
          })),
        ],
      ),
    );

  const cases: { name: string; calls: CoordinatorCall[] }[] = [
    { name: "empty array", calls: [] },
    {
      name: "single approve call",
      calls: [
        {
          target: "0x6c84a8f1c29108F47a79964b5Fe888D4f4D0dE40",
          value: 0n,
          data: "0x095ea7b30000000000000000000000001231deb6f5749ef6ce6943a275a1d3e7486f4eae0000000000000000000000000000000000000000000000000de0b6b3a7640000",
        },
      ],
    },
    {
      name: "approve + LI.FI swap (approval address != router, odd-length blob)",
      calls: [
        {
          target: "0x6c84a8f1c29108F47a79964b5Fe888D4f4D0dE40",
          value: 0n,
          data: "0x095ea7b30000000000000000000000001231deb6f5749ef6ce6943a275a1d3e7486f4eae00000000000000000000000000000000000000000000000000000000000f4240",
        },
        {
          target: "0x1231DEB6f5749EF6cE6943a275A1D3E7486F4EaE",
          value: 0n,
          data: `0x4630a0d8${"ab".repeat(73)}`,
        },
      ],
    },
  ];

  for (const c of cases) {
    it(`matches viem abi.encode — ${c.name}`, () => {
      expect(computeCoordinatorCallsHash(c.calls).toLowerCase()).toBe(
        reference(c.calls).toLowerCase(),
      );
    });
  }
});
