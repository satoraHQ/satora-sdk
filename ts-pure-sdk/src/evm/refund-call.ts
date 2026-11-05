/**
 * The refund calls the server may hand out for an expired EVM lock, decoded
 * so the client can check them and re-encode them itself instead of sending
 * the server's bytes. Any other call is refused: `HTLCErc20` also has a
 * `refund` overload that pays an arbitrary destination, and the server must
 * not be able to pick it (or any other contract call).
 */

import {
  encodeNativeRefundTo,
  encodeRefundTo,
  keccak256,
  NATIVE_TOKEN_ADDRESS,
} from "./coordinator.js";
import { encodeHtlcNativeRefundCallData, normalizeBytes32 } from "./htlc.js";
import { decodeUint256 } from "./wallet.js";

const selector = (signature: string) =>
  keccak256(new TextEncoder().encode(signature)).slice(0, 10);

const SHAPES = {
  // HTLCCoordinator.refundTo: the coordinator is the lock's sender (v6).
  refundTo: {
    selector: selector("refundTo(bytes32,uint256,address,address,uint256)"),
    words: ["hash", "amount", "token", "claim", "timelock"],
  },
  // HTLCNativeCoordinator.refundTo (native v1).
  nativeRefundTo: {
    selector: selector("refundTo(bytes32,uint256,address,uint256)"),
    words: ["hash", "amount", "claim", "timelock"],
  },
  // HTLCErc20.refundFor: pays the lock's sender, the depositor (v7).
  refundFor: {
    selector: selector(
      "refundFor(bytes32,uint256,address,address,address,uint256)",
    ),
    words: ["hash", "amount", "token", "sender", "claim", "timelock"],
  },
  // HTLCNative.refundFor (native v2).
  nativeRefundFor: {
    selector: selector("refundFor(bytes32,uint256,address,address,uint256)"),
    words: ["hash", "amount", "sender", "claim", "timelock"],
  },
  // HTLCNative.refund: msg.sender's own lock, created directly on the HTLC.
  nativeRefund: {
    selector: selector("refund(bytes32,uint256,address,uint256)"),
    words: ["hash", "amount", "claim", "timelock"],
  },
} as const;

export type RefundCallKind = keyof typeof SHAPES;

/** A server refund call, decoded. */
export interface DecodedRefundCall {
  kind: RefundCallKind;
  preimageHash: string;
  amount: bigint;
  /** Lowercase; the zero address for a native lock. */
  token: string;
  /** The lock's sender, for `refundFor`; lowercase. */
  sender?: string;
  claimAddress: string;
  timelock: number;
}

/**
 * Decodes one of the refund calls the server builds, or throws for anything
 * else: an unknown selector, a wrong length, or a dirty address word.
 */
export function decodeRefundCall(data: string): DecodedRefundCall {
  const clean = data.toLowerCase();
  const entry = Object.entries(SHAPES).find(
    ([, shape]) => clean.slice(0, 10) === shape.selector,
  );
  if (!entry) {
    throw new Error(
      `The server's refund call has selector ${clean.slice(0, 10)}, which is none of the refund calls this client accepts`,
    );
  }
  const [kind, shape] = entry as [
    RefundCallKind,
    (typeof SHAPES)[RefundCallKind],
  ];
  if (clean.length !== 10 + 64 * shape.words.length) {
    throw new Error(
      `The server's ${kind} call is ${(clean.length - 10) / 2} bytes of arguments, expected ${32 * shape.words.length}`,
    );
  }
  const values: Record<string, string> = {};
  shape.words.forEach((name, i) => {
    values[name] = clean.slice(10 + 64 * i, 10 + 64 * (i + 1));
  });
  const address = (name: string) => {
    const word = values[name];
    if (!/^0{24}/.test(word)) {
      throw new Error(`The server's ${kind} call has a malformed ${name}`);
    }
    return `0x${word.slice(24)}`;
  };
  const timelock = decodeUint256(values.timelock);
  if (timelock > BigInt(Number.MAX_SAFE_INTEGER)) {
    throw new Error(`The server's ${kind} call has a malformed timelock`);
  }
  return {
    kind,
    preimageHash: `0x${values.hash}`,
    amount: decodeUint256(values.amount),
    token:
      "token" in values ? address("token") : NATIVE_TOKEN_ADDRESS.toLowerCase(),
    sender: "sender" in values ? address("sender") : undefined,
    claimAddress: address("claim"),
    timelock: Number(timelock),
  };
}

/** Encodes `call` on `to` with this client's own encoders. */
export function encodeRefundCall(
  to: string,
  call: DecodedRefundCall,
): { to: string; data: string } {
  const word = (hex: string) => normalizeBytes32(hex);
  const uint = (value: bigint | number) =>
    BigInt(value).toString(16).padStart(64, "0");
  switch (call.kind) {
    case "refundTo":
      return { to, data: encodeRefundTo(to, call).data };
    case "nativeRefundTo":
      return { to, data: encodeNativeRefundTo(to, call).data };
    case "nativeRefund":
      return { to, data: encodeHtlcNativeRefundCallData(to, call).data };
    case "refundFor":
    case "nativeRefundFor": {
      if (call.sender === undefined) {
        throw new Error("refundFor needs the lock's sender");
      }
      const words =
        call.kind === "refundFor"
          ? [
              word(call.preimageHash),
              uint(call.amount),
              word(call.token),
              word(call.sender),
              word(call.claimAddress),
              uint(call.timelock),
            ]
          : [
              word(call.preimageHash),
              uint(call.amount),
              word(call.sender),
              word(call.claimAddress),
              uint(call.timelock),
            ];
      return { to, data: [SHAPES[call.kind].selector, ...words].join("") };
    }
  }
}
