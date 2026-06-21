import { defineConfig } from "tsup";

// Dual ESM + CJS build with dependencies bundled into the output (code-split
// into shared chunks).
//
// Why dual-format: the SDK is authored as ESM, but consumers live in mixed
// environments — CommonJS codebases, older bundlers, and test runners. Shipping
// an ESM build *and* a self-contained CJS build means any of them can load the
// SDK through the matching `exports` condition, with no per-consumer config.
//
// Why bundle the dependencies (rather than leave them external): parts of the
// dependency tree are ESM-only, and some of them are reached through CommonJS
// packages that `require()` those ESM-only modules. Node 22 resolves that
// CJS→ESM `require()` natively, but many CommonJS loaders and test runners
// (e.g. Jest) cannot, so they fail to load the SDK. Bundling resolves every
// such edge at build time and inlines it, so the CJS output is fully
// self-contained and never performs a runtime `require()` of an ESM-only
// package. This also gives consumers a smaller dependency surface and faster
// cold start.
//
// Only deps that genuinely cannot or should not be inlined are kept external:
//  - better-sqlite3: native addon (.node), optional, used by the /node entry
//  - @circle-fin/*: optional peer deps, used only by the /cctp-bridge entry
//  - @react-native-async-storage/async-storage: React Native-only; never
//    loaded in a Node/CJS context
const external = [
  "better-sqlite3",
  "@circle-fin/adapter-viem-v2",
  "@circle-fin/bridge-kit",
  "@react-native-async-storage/async-storage",
];

export default defineConfig({
  entry: {
    index: "src/index.ts",
    node: "src/node.ts",
    delegate: "src/delegate.ts",
    "cctp-bridge/index": "src/cctp-bridge/index.ts",
  },
  format: ["esm", "cjs"],
  // Declarations are emitted separately by `tsc --emitDeclarationOnly` (see the
  // build script) rather than bundled by tsup. tsup's bundled single-file .d.ts
  // drops internal types that are referenced by public signatures but not
  // re-exported from the entry, which breaks downstream `tsc` consumers
  // (TS4023). Emitting the full declaration tree with tsc keeps the exact type
  // surface the package shipped before this build change.
  dts: false,
  splitting: true,
  sourcemap: true,
  clean: true,
  treeshake: true,
  target: "node18",
  // Bundle every dependency by default, then carve out the ones above.
  noExternal: [/.*/],
  external,
});
