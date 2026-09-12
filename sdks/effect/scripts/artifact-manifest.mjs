import { createHash } from "node:crypto"
import { existsSync, readFileSync, readdirSync, renameSync, statSync, writeFileSync } from "node:fs"
import { dirname, join, relative, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { spawnSync } from "node:child_process"
import { templateMatrix } from "./template-matrix.mjs"

export const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..")
export const manifestPath = join(packageDir, "wasm", "artifact-provenance.json")
const bundleFiles = [
  "dist/index.mjs",
  "dist/middleware.mjs",
  "dist/effect.mjs",
  "dist/sqlite.mjs",
  "dist/postgres.mjs",
  "dist/mysql.mjs",
  "dist/ignite.mjs",
]

const inputRoots = [
  "src",
  "wit",
  "golem-types",
  "package.json",
  "package-lock.json",
  "rollup.config.mjs",
  "tsconfig.json",
  "scripts/generate-agent-template.mjs",
  "scripts/compile-agent-templates.mjs",
  "scripts/copy-agent-template.mjs",
  "scripts/template-matrix.mjs",
  "scripts/artifact-manifest.mjs",
]
const outputPaths = [
  ...templateMatrix.map(({ wasmFile }) => join(packageDir, "wasm", wasmFile)),
  ...bundleFiles.map((path) => join(packageDir, path)),
]

export function currentProvenance() {
  return {
    version: 1,
    inputs: hashes(inputRoots.flatMap(filesBelow)),
    toolchain: {
      node: process.version,
      wasmRquickjs: versionOf("wasm-rquickjs", ["--version"]),
      rustc: versionOf("rustc", ["--version", "--verbose"]),
      cargo: versionOf("cargo", ["--version"]),
    },
    outputs: hashes(outputPaths),
  }
}

export function writeProvenance() {
  const invalid = outputPaths.filter((path) => !existsSync(path) || statSync(path).size === 0)
  if (invalid.length > 0) {
    throw new Error(
      `Cannot record provenance for missing or empty artifacts:\n${invalid.join("\n")}`,
    )
  }
  const provenance = currentProvenance()
  const temporary = `${manifestPath}.tmp`
  writeFileSync(temporary, `${JSON.stringify(provenance, null, 2)}\n`)
  renameSync(temporary, manifestPath)
}

function filesBelow(path) {
  const absolute = join(packageDir, path)
  if (!existsSync(absolute)) return [absolute]
  if (!statSync(absolute).isDirectory()) return [absolute]
  return readdirSync(absolute)
    .sort()
    .flatMap((entry) => filesBelow(join(path, entry)))
}

function hashes(paths) {
  return Object.fromEntries(
    [...new Set(paths)].sort().map((path) => {
      const name = relative(packageDir, path)
      return [
        name,
        existsSync(path) ? createHash("sha256").update(readFileSync(path)).digest("hex") : null,
      ]
    }),
  )
}

function versionOf(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" })
  if (result.error) return null
  if (result.status !== 0) return null
  return `${result.stdout}${result.stderr}`.trim()
}
