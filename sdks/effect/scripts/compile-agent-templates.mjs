import { spawnSync } from "node:child_process"
import { existsSync, readFileSync, writeFileSync } from "node:fs"
import { join, resolve } from "node:path"
import { templateMatrix } from "./template-matrix.mjs"

const packageDir = process.cwd()
const targetDir = resolve(process.env.CARGO_TARGET_DIR ?? join(packageDir, ".template-target"))
let canonicalLock

for (const [index, template] of templateMatrix.entries()) {
  const directory = join(packageDir, template.wrapperDirectory)
  const manifest = join(directory, "Cargo.toml")
  const lock = join(directory, "Cargo.lock")
  if (index > 0) {
    const packageLine = `name = "${templateMatrix[0].world}"`
    if (canonicalLock.split(packageLine).length - 1 !== 1)
      throw new Error(`Cannot reuse canonical wrapper lock for ${template.world}`)
    writeFileSync(lock, canonicalLock.replace(packageLine, `name = "${template.world}"`))
  }
  const result = spawnSync(
    "cargo",
    [
      "build",
      ...(existsSync(lock) ? ["--locked"] : []),
      "--target",
      "wasm32-wasip2",
      "--target-dir",
      targetDir,
      "--manifest-path",
      manifest,
      "--release",
      "--no-default-features",
      "--features",
      "full-p3,golem",
    ],
    { stdio: "inherit" },
  )
  if (result.error) throw result.error
  if (result.status !== 0) process.exit(result.status ?? 1)
  if (index === 0) canonicalLock = readFileSync(lock, "utf8")
}
