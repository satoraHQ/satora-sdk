import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const pkg = JSON.parse(await readFile(join(root, "package.json"), "utf8"));
const temp = await mkdtemp(join(tmpdir(), "satora-swap-package-"));
const run = (command, args, cwd = temp) =>
  execFileSync(command, args, { cwd, stdio: "pipe" });

try {
  run("pnpm", ["pack", "--pack-destination", temp], root);
  const tarball = (await readdir(temp)).find((name) => name.endsWith(".tgz"));
  assert.ok(tarball);
  await writeFile(
    join(temp, "package.json"),
    '{"private":true,"type":"module"}\n',
  );
  run("npm", [
    "install",
    "--ignore-scripts",
    "--no-audit",
    "--no-fund",
    "--no-package-lock",
    join(temp, tarball),
    "typescript@^5.7.0",
  ]);
  const installed = JSON.parse(
    await readFile(
      join(temp, "node_modules/@satora/swap/package.json"),
      "utf8",
    ),
  );
  for (const field of [
    "dependencies",
    "optionalDependencies",
    "peerDependencies",
  ]) {
    assert.ok(!installed[field]?.["@lendasat/lendaswap-sdk-pure"]);
  }
  assert.throws(() =>
    run("node", ["-p", "require.resolve('@lendasat/lendaswap-sdk-pure')"]),
  );

  // Exercise every published entry in both module formats, with no workspace
  // links available. Native SQLite is not constructed (install scripts are off).
  const smoke = `
    import assert from "node:assert/strict";
    import { createRequire } from "node:module";
    const require = createRequire(import.meta.url);
    const entries = ["@satora/swap", "@satora/swap/node", "@satora/swap/delegate"];
    if (process.argv.includes("--cctp")) entries.push("@satora/swap/cctp-bridge");
    for (const name of entries) {
      assert.ok(Object.keys(await import(name)).length);
      assert.ok(Object.keys(require(name)).length);
    }
    const { Client, InMemorySwapStorage, InMemoryWalletStorage, SDK_NAME, SDK_VERSION } = await import("@satora/swap");
    assert.equal(SDK_NAME, "swap");
    assert.equal(SDK_VERSION, ${JSON.stringify(pkg.version)});
    assert.equal(typeof Client.builder().withSwapStorage(new InMemorySwapStorage()).withSignerStorage(new InMemoryWalletStorage()).build, "function");
  `;
  await writeFile(join(temp, "smoke.mjs"), smoke);
  // The main SDK, Node storage and delegate entry must load without Circle.
  assert.throws(() =>
    run("node", ["-p", "require.resolve('@circle-fin/bridge-kit')"]),
  );
  run("node", ["smoke.mjs"]);
  run("npm", [
    "install",
    "--ignore-scripts",
    "--no-audit",
    "--no-fund",
    "--no-package-lock",
    ...Object.entries(pkg.peerDependencies).map(
      ([name, version]) => `${name}@${version}`,
    ),
  ]);
  run("node", ["smoke.mjs", "--cctp"]);

  // Re-emit consumer declarations to catch public signatures that refer to
  // private types, as well as imports of the former pure package.
  const consumer = `
    import { Client, type StoredSwap, swapToTracked } from "@satora/swap";
    import { SqliteSwapStorage } from "@satora/swap/node";
    import * as delegate from "@satora/swap/delegate";
    import * as bridge from "@satora/swap/cctp-bridge";
    export const builder = Client.builder();
    export const swapStorage = new SqliteSwapStorage(":memory:");
    export const quote = (client: Client) => client.getQuote;
    export const track = (stored: StoredSwap) => swapToTracked(stored);
    export { delegate, bridge };
  `;
  await writeFile(join(temp, "consumer.mts"), consumer);
  await writeFile(join(temp, "consumer.cts"), consumer);
  run("node", [
    "node_modules/typescript/bin/tsc",
    "--module",
    "NodeNext",
    "--moduleResolution",
    "NodeNext",
    "--target",
    "ES2022",
    "--strict",
    "--skipLibCheck",
    "--declaration",
    "--emitDeclarationOnly",
    "consumer.mts",
    "consumer.cts",
  ]);
  console.log(
    "Packed @satora/swap: ESM, CJS, all subpaths and consumer declarations passed without pure installed.",
  );
} catch (error) {
  if (error.stdout) process.stderr.write(error.stdout);
  if (error.stderr) process.stderr.write(error.stderr);
  throw error;
} finally {
  await rm(temp, { recursive: true, force: true });
}
