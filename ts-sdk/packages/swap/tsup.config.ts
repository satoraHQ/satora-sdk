import { defineConfig } from "tsup";

// Dual ESM + CJS build including the internal pure SDK.
//
// Why dual-format: consumers live in mixed environments — ESM codebases,
// CommonJS codebases, and test runners. Shipping both an ESM and a CJS build
// lets each load this package through the matching `exports` condition with no
// per-consumer configuration.
//
// Pure is a private build dependency. Inline its already-built JS (including
// shared chunks) so consumers never resolve the legacy package from npm.
const external = [
  "better-sqlite3",
  "@circle-fin/adapter-viem-v2",
  "@circle-fin/bridge-kit",
  "@react-native-async-storage/async-storage",
  "node-gyp-build",
  "bufferutil",
  "utf-8-validate",
];

export default defineConfig({
  entry: {
    index: "src/index.ts",
    node: "src/node.ts",
    delegate: "src/delegate.ts",
    "cctp-bridge": "src/cctp-bridge.ts",
  },
  format: ["esm", "cjs"],
  noExternal: [/^@lendasat\/lendaswap-sdk-pure(?:\/|$)/],
  external,
  esbuildOptions(options) {
    // Keep native addons and optional peers external when inlining pure's
    // prebuilt chunks, including imports originally resolved from its directory.
    options.external = [...(options.external ?? []), ...external];
  },
  // Preserve the full declaration tree: bundled declarations can lose internal
  // types referenced by public signatures. See scripts/package-types.mjs.
  dts: false,
  splitting: true,
  sourcemap: true,
  clean: true,
  target: "node18",
});
