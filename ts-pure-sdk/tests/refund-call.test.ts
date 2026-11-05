import { encodeFunctionData, parseAbi } from "viem";

/** viem's encoder for one human-readable signature, loosely typed. */
const encode = (signature: string, args: readonly unknown[]): string =>
  (encodeFunctionData as (p: unknown) => string)({
    abi: parseAbi([signature] as never),
    args,
  });

import { describe, expect, it } from "vitest";
import { decodeRefundCall, encodeRefundCall } from "../src/evm/refund-call.js";

const HASH = `0x${"11".repeat(32)}` as const;
const TOKEN = "0x1bfd67037b42cf73acf2047067bd4f2c47d9bfd6" as const;
const SENDER = "0x90f79bf6eb2c4f870365e785982e1f101e93b906" as const;
const CLAIM = "0xfcb0b3c3c6320420b749bcd3e4400f0b9fdf682a" as const;
const TO = "0x5555555555555555555555555555555555555555";

// Every call the server's refund-and-swap-calldata endpoint builds
// (swap/src/api/refund_and_swap_calldata.rs), encoded independently.
const SERVER_CALLS: [string, string, readonly unknown[]][] = [
  [
    "refundTo",
    "function refundTo(bytes32,uint256,address,address,uint256)",
    [HASH, 7n, TOKEN, CLAIM, 1_800_000_000n],
  ],
  [
    "nativeRefundTo",
    "function refundTo(bytes32,uint256,address,uint256)",
    [HASH, 7n, CLAIM, 1_800_000_000n],
  ],
  [
    "refundFor",
    "function refundFor(bytes32,uint256,address,address,address,uint256)",
    [HASH, 7n, TOKEN, SENDER, CLAIM, 1_800_000_000n],
  ],
  [
    "nativeRefundFor",
    "function refundFor(bytes32,uint256,address,address,uint256)",
    [HASH, 7n, SENDER, CLAIM, 1_800_000_000n],
  ],
  [
    "nativeRefund",
    "function refund(bytes32,uint256,address,uint256)",
    [HASH, 7n, CLAIM, 1_800_000_000n],
  ],
];

describe("decodeRefundCall", () => {
  it.each(
    SERVER_CALLS,
  )("decodes and re-encodes the server's %s", (kind, signature, args) => {
    const data = encode(signature, args);
    const call = decodeRefundCall(data);
    expect(call.kind).toBe(kind);
    expect(call.claimAddress).toBe(CLAIM);
    expect(call.timelock).toBe(1_800_000_000);
    expect(encodeRefundCall(TO, call)).toEqual({ to: TO, data });
  });

  it("refuses a dirty address word", () => {
    const data = encode(SERVER_CALLS[2][1], SERVER_CALLS[2][2]);
    // Set a high byte of the sender word.
    const dirty = `${data.slice(0, 10 + 64 * 3)}ff${data.slice(12 + 64 * 3)}`;
    expect(() => decodeRefundCall(dirty)).toThrow("malformed sender");
  });
});
