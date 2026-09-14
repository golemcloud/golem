import { rmSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { templateMatrix } from "./template-matrix.mjs"

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..")
for (const path of [
  "dist",
  ".generated-types",
  ".template-target",
  ...templateMatrix.map(({ wrapperDirectory }) => wrapperDirectory),
  ...templateMatrix.map(({ wasmFile }) => join("wasm", wasmFile)),
]) {
  rmSync(join(packageDir, path), { recursive: true, force: true })
}
