// Strip trailing whitespace from the package changelogs after `changeset version`.
//
// Changesets 3 formats the changelogs it writes with the project's formatter,
// which it detects as dprint (repo-root dprint.json) and runs via
// `pnpm exec dprint`. dprint is not an npm dependency here, so that fails.
// `.changeset/config.json` sets `"format": false` instead, and this removes
// the whitespace-only continuation lines Changesets leaves behind, the only
// thing CI's `dprint check` rejects in the generated changelogs.
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const packagesDir = new URL("../packages/", import.meta.url).pathname;

for (const pkg of readdirSync(packagesDir)) {
  const file = join(packagesDir, pkg, "CHANGELOG.md");
  if (!existsSync(file)) continue;
  const before = readFileSync(file, "utf8");
  const after = before.replace(/[ \t]+$/gm, "");
  if (after !== before) {
    writeFileSync(file, after);
    console.log(`✓ trimmed trailing whitespace in ${pkg}/CHANGELOG.md`);
  }
}
