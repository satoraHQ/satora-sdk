import { describe, expect, it } from "vitest";
import {
  classifyDestination,
  isArkadeAddress,
  isBolt11,
  isLightningDestination,
  isLnAddress,
  isLnurl,
  toLightningDestination,
} from "./destination.js";

// Representative fixtures.
const BOLT11_MAINNET = "lnbc100u1p3pj257pp5...";
const BOLT11_TESTNET = "lntb100u1p3pj257pp5...";
const BOLT11_SIGNET = "lntbs100u1p3pj257pp5...";
const BOLT11_REGTEST = "lnbcrt100u1p3pj257pp5...";
const LNURL =
  "lnurl1dp68gurn8ghj7um9wfmxjcm99e3k7mf0v9cxj0m385ekvcenxc6r2c35xvukxefcv5mkvv34x5ekzd3ev56nyd3hxqurzepexejxxepnxscrvwfnv9nxzcn9xq6xyefhvgcxxcmyxymnserxfq5fns";
const LN_ADDRESS = "refund@lnurl.mutinynet.com";
const ARK_MAINNET =
  "ark1qqellv77udfmr20tun8dvju5vgudpf9vxe8jwhthrkn26fz96pawqfdy8nz";
const ARK_TEST =
  "tark1qra883hysahlkt0ujcwhv0x2n278849c3m7t3a08l7fdc40f4f2nmsulchcsl8st7r";
const L1_BECH32 = "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq";
const L1_TESTNET = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";
const L1_REGTEST = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";
const L1_LEGACY = "1BVxNn3T12veSK6DgqwU4Hdn7QHcDDRag7";
// Taproot (bc1p): bech32m, same encoding family as Ark addresses — a good check
// that classification keys on the hrp, not the encoding.
const L1_TAPROOT =
  "bc1p6s0nwxu3fhxkffdrddc5ze3qxfjvldkgtk4z0c4cvlql5ak4xqwqfwqd3v";

describe("isBolt11", () => {
  it("matches BOLT11 on every network", () => {
    expect(isBolt11(BOLT11_MAINNET)).toBe(true);
    expect(isBolt11(BOLT11_TESTNET)).toBe(true);
    expect(isBolt11(BOLT11_SIGNET)).toBe(true);
    expect(isBolt11(BOLT11_REGTEST)).toBe(true);
    expect(isBolt11("LNBC100U1P3PJ257")).toBe(true); // case-insensitive
  });
  it("rejects non-invoices", () => {
    expect(isBolt11(LNURL)).toBe(false);
    expect(isBolt11(LN_ADDRESS)).toBe(false);
    expect(isBolt11(ARK_TEST)).toBe(false);
    expect(isBolt11(L1_BECH32)).toBe(false);
  });
});

describe("isLnurl", () => {
  it("matches lnurl1 strings (case-insensitive)", () => {
    expect(isLnurl(LNURL)).toBe(true);
    expect(isLnurl(LNURL.toUpperCase())).toBe(true);
  });
  it("rejects everything else", () => {
    expect(isLnurl(BOLT11_MAINNET)).toBe(false);
    expect(isLnurl(LN_ADDRESS)).toBe(false);
    expect(isLnurl(ARK_TEST)).toBe(false);
    expect(isLnurl("lnurl")).toBe(false); // no bech32 body
  });
});

describe("isLnAddress", () => {
  it("matches user@host.tld", () => {
    expect(isLnAddress(LN_ADDRESS)).toBe(true);
    expect(isLnAddress("alice@walletofsatoshi.com")).toBe(true);
  });
  it("rejects malformed / non-addresses", () => {
    expect(isLnAddress("user@host")).toBe(false); // no dot
    expect(isLnAddress("@host.com")).toBe(false); // no local part
    expect(isLnAddress("a@b@c.com")).toBe(false); // two @
    expect(isLnAddress(BOLT11_MAINNET)).toBe(false);
    expect(isLnAddress(ARK_TEST)).toBe(false);
  });
});

describe("isLightningDestination", () => {
  it("is true for invoice / lnurl / address", () => {
    expect(isLightningDestination(BOLT11_MAINNET)).toBe(true);
    expect(isLightningDestination(LNURL)).toBe(true);
    expect(isLightningDestination(LN_ADDRESS)).toBe(true);
    expect(isLightningDestination(`  ${BOLT11_MAINNET}  `)).toBe(true); // trims
  });
  it("is false for ark / onchain", () => {
    expect(isLightningDestination(ARK_TEST)).toBe(false);
    expect(isLightningDestination(L1_BECH32)).toBe(false);
    expect(isLightningDestination(L1_LEGACY)).toBe(false);
  });
});

describe("isArkadeAddress", () => {
  it("matches ark1 (mainnet) and tark1 (test) bech32m", () => {
    expect(isArkadeAddress(ARK_MAINNET)).toBe(true);
    expect(isArkadeAddress(ARK_TEST)).toBe(true);
    expect(isArkadeAddress(ARK_TEST.toUpperCase())).toBe(true);
    expect(isArkadeAddress(`  ${ARK_TEST}  `)).toBe(true); // trims
  });
  it("does not match onchain or lightning", () => {
    expect(isArkadeAddress(L1_BECH32)).toBe(false); // bc1, not ark1
    expect(isArkadeAddress(L1_TESTNET)).toBe(false); // tb1, not tark1
    expect(isArkadeAddress(L1_TAPROOT)).toBe(false); // bc1p bech32m, not ark1
    expect(isArkadeAddress(L1_LEGACY)).toBe(false); // base58
    expect(isArkadeAddress(BOLT11_MAINNET)).toBe(false);
  });
});

describe("classifyDestination", () => {
  it("routes lightning destinations", () => {
    expect(classifyDestination(BOLT11_MAINNET)).toBe("lightning");
    expect(classifyDestination(LNURL)).toBe("lightning");
    expect(classifyDestination(LN_ADDRESS)).toBe("lightning");
  });
  it("routes arkade addresses", () => {
    expect(classifyDestination(ARK_MAINNET)).toBe("arkade");
    expect(classifyDestination(ARK_TEST)).toBe("arkade");
  });
  it("routes everything else to l1", () => {
    expect(classifyDestination(L1_BECH32)).toBe("l1");
    expect(classifyDestination(L1_TESTNET)).toBe("l1");
    expect(classifyDestination(L1_REGTEST)).toBe("l1");
    expect(classifyDestination(L1_LEGACY)).toBe("l1"); // base58 P2PKH
    expect(classifyDestination(L1_TAPROOT)).toBe("l1"); // bc1p taproot
  });
  it("trims surrounding whitespace before classifying", () => {
    expect(classifyDestination(`  ${ARK_TEST}\n`)).toBe("arkade");
  });
});

describe("toLightningDestination", () => {
  it("maps a BOLT11 invoice and ignores amountSats", () => {
    expect(toLightningDestination(BOLT11_MAINNET)).toEqual({
      lightningInvoice: BOLT11_MAINNET,
    });
    expect(toLightningDestination(BOLT11_MAINNET, 1000)).toEqual({
      lightningInvoice: BOLT11_MAINNET,
    });
  });
  it("maps a Lightning address with amountSats", () => {
    expect(toLightningDestination(LN_ADDRESS, 5000)).toEqual({
      lightningAddress: LN_ADDRESS,
      amountSats: 5000,
    });
  });
  it("maps an LNURL with amountSats", () => {
    expect(toLightningDestination(LNURL, 5000)).toEqual({
      lnurl: LNURL,
      amountSats: 5000,
    });
  });
  it("requires amountSats for address / lnurl", () => {
    expect(() => toLightningDestination(LN_ADDRESS)).toThrow(
      /amountSats is required/,
    );
    expect(() => toLightningDestination(LNURL)).toThrow(
      /amountSats is required/,
    );
  });
  it("rejects a non-Lightning destination", () => {
    expect(() => toLightningDestination(ARK_TEST, 5000)).toThrow(
      /unrecognized/,
    );
    expect(() => toLightningDestination(L1_BECH32, 5000)).toThrow(
      /unrecognized/,
    );
  });
  it("trims whitespace", () => {
    expect(toLightningDestination(`  ${BOLT11_MAINNET} `)).toEqual({
      lightningInvoice: BOLT11_MAINNET,
    });
  });
});
