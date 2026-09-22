import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createApiClient,
  PROTOCOL_HEADERS,
  PROTOCOL_VERSIONS,
  ProtocolComponent,
} from "../src/index.js";

/**
 * The protocol epoch headers are a contract between three codebases: this
 * SDK, the Rust SDK and the server that validates them. Nothing shares the
 * names at build time, so these tests read the other two sources and fail the
 * moment one side renames, adds or drops a component without the others.
 */

const repoFile = (relative: string) =>
  readFileSync(
    fileURLToPath(new URL(`../../../${relative}`, import.meta.url)),
    "utf8",
  );

const headerNames = (source: string) =>
  [...source.matchAll(/"(x-satora-[a-z0-9-]+-version)"/g)].map(
    ([, name]) => name,
  );

describe("protocol epoch headers", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("declares one header per component", () => {
    expect(Object.keys(PROTOCOL_HEADERS)).toHaveLength(
      Object.values(ProtocolComponent).length,
    );
  });

  it("uses the header names the server validates", () => {
    // Component::header in the server's protocol_compat module. The rest of
    // that file also names the legacy x-satora-server-version header.
    const source = repoFile("swap/src/protocol_compat.rs");
    const body = source.match(
      /pub fn header\(self\) -> &'static str \{([\s\S]*?)\n {4}\}/,
    );
    expect(body, "server Component::header not found").not.toBeNull();
    const server = headerNames(body?.[1] ?? "");
    for (const name of Object.keys(PROTOCOL_HEADERS)) {
      expect(server).toContain(name);
    }
    expect(new Set(server)).toEqual(new Set(Object.keys(PROTOCOL_HEADERS)));
  });

  it("sends the same headers and epochs as the Rust SDK", () => {
    const source = repoFile("client-sdk/rust-sdk/src/client.rs");
    const table = source.match(
      /PROTOCOL_HEADERS: \[\(&str, &str\); \d+\] = \[([\s\S]*?)\];/,
    );
    expect(table, "Rust SDK PROTOCOL_HEADERS table not found").not.toBeNull();
    const rust = Object.fromEntries(
      [...(table?.[1] ?? "").matchAll(/\("([^"]+)", "([^"]+)"\)/g)].map(
        ([, name, epoch]) => [name, epoch],
      ),
    );
    expect(rust).toEqual(PROTOCOL_HEADERS);
  });

  it("sends each component's epoch as its header value", () => {
    for (const component of Object.values(ProtocolComponent)) {
      expect(PROTOCOL_HEADERS[`x-satora-${component}-version`]).toBe(
        String(PROTOCOL_VERSIONS[component]),
      );
    }
  });

  it("attaches every header to API client requests", async () => {
    const fetchMock = vi.fn(
      async (_request: Request) =>
        new Response("[]", {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const client = createApiClient({ baseUrl: "https://example.test" });
    await client.GET("/tokens");

    expect(fetchMock).toHaveBeenCalledOnce();
    const request = fetchMock.mock.calls[0][0];
    for (const [name, value] of Object.entries(PROTOCOL_HEADERS)) {
      expect(request.headers.get(name)).toBe(value);
    }
  });
});
