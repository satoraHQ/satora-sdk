import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const dist = join(root, "dist");
const esm = join(dist, "types/esm");
const pureDist = resolve(root, "../../../ts-pure-sdk/dist");

async function declarations(dir) {
  const files = await readdir(dir, { withFileTypes: true });
  const nested = await Promise.all(
    files.map(async (file) => {
      const path = join(dir, file.name);
      return file.isDirectory()
        ? declarations(path)
        : path.endsWith(".d.ts")
          ? [path]
          : [];
    }),
  );
  return nested.flat();
}

async function write(path, contents) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, contents);
}

// Keep pure's complete declaration tree, including types that are used by a
// public signature but are not exported from its entry points.
for (const source of await declarations(pureDist)) {
  const contents = (await readFile(source, "utf8")).replace(
    /^\/\/# sourceMappingURL=.*$/gm,
    "",
  );
  await write(join(esm, "pure", relative(pureDist, source)), contents);
}

const entries = {
  index: "index",
  node: "node",
  delegate: "delegate",
  "cctp-bridge": "cctp-bridge/index",
};
for (const path of await declarations(esm)) {
  let contents = await readFile(path, "utf8");
  contents = contents.replace(
    /(["'])@lendasat\/lendaswap-sdk-pure(?:\/([^"']+))?\1/g,
    (_, quote, entry = "index") => {
      if (!(entry in entries))
        throw new Error(`Unknown pure entry point: ${entry}`);
      let local = relative(
        dirname(path),
        join(esm, "pure", `${entries[entry]}.js`),
      );
      if (!local.startsWith(".")) local = `./${local}`;
      return `${quote}${local}${quote}`;
    },
  );
  await writeFile(path, contents);
  // CJS consumers need a CJS declaration tree as well; otherwise NodeNext
  // resolves an ESM declaration from a .d.cts entry point.
  const cjsPath = join(dist, "types/cjs", relative(esm, path)).replace(
    /\.d\.ts$/,
    ".d.cts",
  );
  await write(
    cjsPath,
    contents.replace(/(["'])(\.[^"']*)\.js\1/g, "$1$2.cjs$1"),
  );
}

for (const entry of Object.keys(entries)) {
  await write(
    join(dist, `${entry}.d.ts`),
    `export * from "./types/esm/swap/${entry}.js";\n`,
  );
  await write(
    join(dist, `${entry}.d.cts`),
    `export * from "./types/cjs/swap/${entry}.cjs";\n`,
  );
}
