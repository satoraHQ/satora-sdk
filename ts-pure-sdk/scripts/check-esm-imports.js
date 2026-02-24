#!/usr/bin/env node

/**
 * Checks that all relative imports in TypeScript source files include the .js
 * extension, which is required for Node.js ESM resolution.
 *
 * Run: node scripts/check-esm-imports.js
 */

import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";

const SRC_DIR = new URL("../src", import.meta.url).pathname;

// Matches: from "./foo" or from "../bar/baz" WITHOUT a .js extension
const BAD_IMPORT_RE =
  /(?:from|import)\s+["'](\.\.?\/[^"']+)["']/g;

async function* walkTs(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      yield* walkTs(full);
    } else if (entry.name.endsWith(".ts") && !entry.name.endsWith(".d.ts")) {
      yield full;
    }
  }
}

let errors = 0;

for await (const file of walkTs(SRC_DIR)) {
  const content = await readFile(file, "utf8");
  const lines = content.split("\n");

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    let match;
    BAD_IMPORT_RE.lastIndex = 0;
    while ((match = BAD_IMPORT_RE.exec(line)) !== null) {
      const specifier = match[1];
      // Allow non-relative (bare) specifiers and .js imports
      if (!specifier.endsWith(".js")) {
        const rel = relative(process.cwd(), file);
        console.error(
          `ERROR: ${rel}:${i + 1} — relative import missing .js extension: "${specifier}"`,
        );
        errors++;
      }
    }
  }
}

if (errors > 0) {
  console.error(`\nFound ${errors} relative import(s) missing .js extension.`);
  process.exit(1);
} else {
  console.log("All relative imports have .js extensions. ✓");
}
