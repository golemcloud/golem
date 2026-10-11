import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { rollup } from "rollup";

const root = path.resolve(import.meta.dirname, "..");
const output = path.join(root, "golem-temp/isomorphic-git");
const source = path.join(output, "source");
const provenance = JSON.parse(
  await fs.readFile(path.join(root, "provenance.json"), "utf8"),
).isomorphicGit;
const hash = (bytes) => crypto.createHash("sha256").update(bytes).digest("hex");
const patch = await fs.readFile(path.join(root, provenance.patch));
if (hash(patch) !== provenance.patchSha256) {
  throw new Error("isomorphic-git source patch differs from its provenance");
}
const stamp = hash(
  Buffer.concat([
    Buffer.from(JSON.stringify(provenance)),
    patch,
    await fs.readFile(import.meta.filename),
    await fs.readFile(path.join(root, "package-lock.json")),
  ]),
);
if (
  (await fs.readFile(path.join(output, "stamp"), "utf8").catch(() => "")) ===
  stamp
) {
  process.exit(0);
}
const archive = path.join(root, "golem-temp/isomorphic-git-source.tar.gz");
await fs.mkdir(path.dirname(archive), { recursive: true });
let bytes = await fs.readFile(archive).catch(() => undefined);
if (!bytes) {
  const response = await fetch(provenance.sourceArchive);
  if (!response.ok)
    throw new Error(`source download failed: ${response.status}`);
  bytes = Buffer.from(await response.arrayBuffer());
}
if (hash(bytes) !== provenance.sourceArchiveSha256) {
  throw new Error("isomorphic-git source archive checksum mismatch");
}
await fs.writeFile(archive, bytes);
await fs.rm(output, { recursive: true, force: true });
await fs.mkdir(source, { recursive: true });
function exec(command, args, cwd = root) {
  const result = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (result.error || result.status !== 0) {
    throw result.error ?? new Error(result.stderr || `${command} failed`);
  }
}
exec("tar", ["-xzf", archive, "--strip-components=1", "-C", source]);
// Apply from the repository root: git apply can silently skip paths when run
// from a subdirectory, even with a --directory argument.
const checkout = spawnSync("git", ["rev-parse", "--show-toplevel"], {
  cwd: root,
  encoding: "utf8",
});
if (checkout.status !== 0)
  throw new Error("source build requires a Git checkout");
const checkoutRoot = checkout.stdout.trim();
const directory = path.relative(checkoutRoot, source).split(path.sep).join("/");
exec(
  "git",
  ["apply", `--directory=${directory}`, path.join(root, provenance.patch)],
  checkoutRoot,
);
const patched = await fs.readFile(
  path.join(source, "src/commands/fetch.js"),
  "utf8",
);
if (
  !patched.includes("objectsOnly") ||
  !patched.includes("Git pack checksum mismatch")
) {
  throw new Error("isomorphic-git transfer patch was not applied");
}
const manifest = JSON.parse(
  await fs.readFile(path.join(source, "package.json"), "utf8"),
);
const external = (name) => !name.startsWith(".") && !path.isAbsolute(name);
for (const entry of ["index", "models/index", "managers/index"]) {
  const bundle = await rollup({
    input: path.join(source, "src", `${entry}.js`),
    external,
    plugins: [
      {
        name: "pinned-isomorphic-git-version",
        transform(code, id) {
          if (id !== path.join(source, "src/utils/pkg.js")) return null;
          return code.replaceAll("0.0.0-development", provenance.version);
        },
      },
    ],
  });
  try {
    await bundle.write({
      file: path.join(output, `${entry}.js`),
      format: "es",
    });
  } finally {
    await bundle.close();
  }
}
await fs.writeFile(
  path.join(output, "package.json"),
  JSON.stringify({
    name: manifest.name,
    version: `${provenance.version}-golem`,
    type: "module",
  }),
);
await fs.writeFile(
  path.join(output, "tsconfig.json"),
  JSON.stringify({
    files: ["index.js", "models/index.js", "managers/index.js"],
    compilerOptions: {
      allowJs: true,
      declaration: true,
      emitDeclarationOnly: true,
      skipLibCheck: true,
      strictNullChecks: true,
      types: [],
    },
  }),
);
exec(process.execPath, [
  path.join(root, "node_modules/typescript/bin/tsc"),
  "-p",
  path.join(output, "tsconfig.json"),
]);
await fs.copyFile(
  path.join(source, "LICENSE.md"),
  path.join(output, "LICENSE.md"),
);
await fs.writeFile(path.join(output, "stamp"), stamp);
console.log(
  `built isomorphic-git ${provenance.sourceCommit} + ${provenance.patchSha256}`,
);
