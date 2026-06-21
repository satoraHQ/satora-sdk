import { defineConfig } from "tsup";

// Dual ESM + CJS build for the thin re-export layer.
//
// Why dual-format: consumers live in mixed environments — ESM codebases,
// CommonJS codebases, and test runners. Shipping both an ESM and a CJS build
// lets each load this package through the matching `exports` condition with no
// per-consumer configuration.
//
// The underlying SDK is kept external (it is a regular dependency): consumers
// resolve it through its own dual-format `exports`, so this wrapper stays a
// tiny re-export shim and there is a single copy of the SDK in the tree.
export default defineConfig({
  entry: {
    index: "src/index.ts",
    node: "src/node.ts",
    delegate: "src/delegate.ts",
    "cctp-bridge": "src/cctp-bridge.ts",
  },
  format: ["esm", "cjs"],
  dts: true,
  sourcemap: true,
  clean: true,
  target: "node18",
});
