import { afterEach, describe, expect, it, vi } from "vitest";
import {
  CctpForwardingFailedError,
  trackCctpMessage,
} from "../src/cctp/attestation.js";
import type { CctpMessageStatus } from "../src/cctp/types.js";

const TX = `0x${"ab".repeat(32)}`;
const MINT = `0x${"cd".repeat(32)}`;

/** One IRIS message entry; `extra` overrides the delivery fields. */
function message(extra: Record<string, unknown>) {
  return {
    attestation: "0x01",
    decodedMessage: {
      decodedMessageBody: {
        amount: "25000000",
        feeExecuted: "3500",
        maxFee: "4200",
        mintRecipient: "0xrecipient",
      },
      destinationDomain: "7",
      sourceDomain: "3",
    },
    eventNonce: "0x02",
    message: "0x00",
    status: "complete",
    ...extra,
  };
}

/** Serve each response in turn, repeating the last one. */
function serve(...responses: unknown[]) {
  let call = 0;
  const fetchMock = vi.fn(async () => {
    const body = responses[Math.min(call++, responses.length - 1)];
    return new Response(JSON.stringify(body), { status: 200 });
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function track(statuses: CctpMessageStatus[] = []) {
  return trackCctpMessage({
    onStatusChange: (status) => statuses.push(status),
    pollIntervalMs: 1,
    sourceChain: "Arbitrum",
    timeoutMs: 2_000,
    txHash: TX,
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("trackCctpMessage", () => {
  it("completes on Circle's forward", async () => {
    serve({
      messages: [message({ forwardState: "COMPLETE", forwardTxHash: MINT })],
    });
    const result = await track();
    expect(result.status).toBe("COMPLETE");
    expect(result.forwardTxHash).toBe(MINT);
    expect(result.amount).toBe("24996500");
  });

  it("completes on the mint of a message nobody asked Circle to forward", async () => {
    const statuses: CctpMessageStatus[] = [];
    serve(
      // Attested, not minted yet: the server's relayer is about to.
      { messages: [message({ forwardState: null, forwardTxHash: null })] },
      {
        messages: [
          message({
            destinationMintTxHash: MINT,
            forwardState: null,
            forwardTxHash: null,
          }),
        ],
      },
    );
    const result = await track(statuses);
    expect(result.status).toBe("COMPLETE");
    expect(result.forwardTxHash).toBe(MINT);
    expect(statuses).toEqual(["FORWARDING", "COMPLETE"]);
  });

  it("does not take a pending forward's mint field for delivery", async () => {
    const fetchMock = serve(
      { messages: [message({ forwardState: "PENDING" })] },
      {
        messages: [
          message({
            destinationMintTxHash: MINT,
            forwardState: "COMPLETE",
            forwardTxHash: MINT,
          }),
        ],
      },
    );
    const result = await track();
    expect(result.forwardTxHash).toBe(MINT);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("fails on a forward Circle gave up on", async () => {
    serve({ messages: [message({ forwardState: "FAILED" })] });
    await expect(track()).rejects.toBeInstanceOf(CctpForwardingFailedError);
  });
});
