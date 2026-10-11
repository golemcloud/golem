import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";

const root = path.resolve(import.meta.dirname, "..");
const output = path.join(root, "golem-temp/xdiff");
const provenance = JSON.parse(
  await fs.readFile(path.join(root, "provenance.json"), "utf8"),
).xdiff;
const hash = (bytes) => crypto.createHash("sha256").update(bytes).digest("hex");
const compiler = path.join(
  process.env.WASI_SDK_PATH ?? "/opt/wasi-sdk",
  "bin/clang",
);
function exec(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.error || result.status !== 0)
    throw result.error ?? new Error(result.stderr || `${command} failed`);
  return result.stdout;
}
const version = exec(compiler, ["--version"]);
if (!version.includes(provenance.compiler))
  throw new Error("xdiff compiler differs from pinned provenance");
const binaryen = JSON.parse(
  await fs.readFile(
    path.join(root, "node_modules/binaryen/package.json"),
    "utf8",
  ),
);
if (binaryen.version !== provenance.binaryenVersion)
  throw new Error("xdiff transpiler differs from pinned provenance");
await fs.mkdir(output, { recursive: true });
const archive = path.join(output, "git-source.tar.xz");
let bytes = await fs.readFile(archive).catch((error) => {
  if (error.code === "ENOENT") return undefined;
  throw error;
});
if (!bytes) {
  const response = await fetch(provenance.sourceArchive);
  if (!response.ok)
    throw new Error(`xdiff source download failed: ${response.status}`);
  bytes = Buffer.from(await response.arrayBuffer());
}
if (hash(bytes) !== provenance.sourceArchiveSha256)
  throw new Error("xdiff source checksum mismatch");
await fs.writeFile(archive, bytes);
const sources = [
  "xdiffi",
  "xprepare",
  "xutils",
  "xemit",
  "xpatience",
  "xhistogram",
];
exec("tar", [
  "-xJf",
  archive,
  "--strip-components=2",
  "-C",
  output,
  `git-${provenance.version}/xdiff`,
]);
exec(compiler, [
  "-O2",
  "-std=gnu11",
  "-fno-builtin",
  "-nostdlib",
  `-I${path.join(root, "xdiff")}`,
  `-I${output}`,
  path.join(root, "xdiff/normalized-diff.c"),
  ...sources.map((source) => path.join(output, `${source}.c`)),
  "-Wl,--no-entry",
  "-Wl,--export=allocate",
  "-Wl,--export=reset",
  "-Wl,--export=normalized_diff",
  "-Wl,--export-memory",
  "-Wl,--initial-memory=41943040",
  "-Wl,--max-memory=41943040",
  "-o",
  path.join(output, "xdiff.wasm"),
]);
exec(process.execPath, [
  path.join(root, "node_modules/binaryen/bin/wasm2js"),
  path.join(output, "xdiff.wasm"),
  "-O2",
  "--pedantic",
  "-o",
  path.join(output, "compiled.mjs"),
]);
const generated = await fs.readFile(path.join(output, "compiled.mjs"), "utf8");
if (/^import /m.test(generated))
  throw new Error("xdiff must have no platform imports");
const exported = [...generated.matchAll(/^export var (\w+) = /gm)].map(
  (match) => match[1],
);
if (exported.sort().join(",") !== "allocate,memory,normalized_diff,reset")
  throw new Error(`unexpected xdiff exports: ${exported}`);
const javascript =
  `// Generated from pinned LGPL-2.1-or-later / EDL-1.0 xdiff sources.\n` +
  `export function createXdiff() {\n${generated.replaceAll(/^export var /gm, "var ")}\n` +
  `return { ${exported.join(", ")} };\n}\n`;
if (hash(javascript) !== provenance.generatedJavaScriptSha256)
  throw new Error("xdiff generated JavaScript differs from pinned provenance");
await fs.writeFile(path.join(output, "index.js"), javascript);
await fs.writeFile(
  path.join(output, "index.d.ts"),
  `export function createXdiff(): { memory: { buffer: ArrayBuffer }; reset(): void; allocate(size: number): number; normalized_diff(before: number, beforeSize: number, after: number, afterSize: number, output: number, capacity: number): number };\n`,
);
console.log(
  `built import-free xdiff ${provenance.version} as lazy pure JavaScript`,
);
