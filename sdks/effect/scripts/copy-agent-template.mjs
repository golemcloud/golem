import { copyFileSync, existsSync, mkdirSync } from "node:fs"
import { join, resolve } from "node:path"
import { templateMatrix } from "./template-matrix.mjs"
import { writeProvenance } from "./artifact-manifest.mjs"

const packageDir = process.cwd()
const targetDir = resolve(process.env.CARGO_TARGET_DIR ?? join(packageDir, ".template-target"))
for (const template of templateMatrix) {
  const source = join(targetDir, "wasm32-wasip2", "release", template.cargoArtifact)
  const destination = join(packageDir, "wasm", template.wasmFile)
  if (!existsSync(source)) throw new Error(`Built wrapper artifact not found: ${source}`)
  mkdirSync(join(packageDir, "wasm"), { recursive: true })
  copyFileSync(source, destination)
}
writeProvenance()
