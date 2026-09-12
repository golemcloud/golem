import { spawnSync } from "node:child_process"
import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
} from "node:fs"
import { dirname, join, relative, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { templateMatrix } from "./template-matrix.mjs"

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const typesDir = resolve(process.env.GOLEM_DTS_OUTPUT ?? join(packageDir, "golem-types"))
const temporaryDir = `${typesDir}.working-${process.pid}`
const mergedDir = join(temporaryDir, "merged")

function filesBelow(root) {
  return readdirSync(root).flatMap((entry) => {
    const path = join(root, entry)
    return statSync(path).isDirectory() ? filesBelow(path) : [path]
  })
}

rmSync(temporaryDir, { recursive: true, force: true })
try {
  for (const [index, template] of templateMatrix.entries()) {
    const output = index === 0 ? mergedDir : join(temporaryDir, template.role)
    const result = spawnSync(
      "wasm-rquickjs",
      [
        "generate-dts",
        "--wit",
        join(packageDir, "wit"),
        "--output",
        output,
        "--world",
        template.world,
        "--target",
        "wasi-p3",
      ],
      { stdio: "inherit" },
    )
    if (result.error) throw result.error
    if (result.status !== 0) process.exit(result.status ?? 1)
    if (index === 0) continue

    for (const candidate of filesBelow(output).filter(
      (path) => relative(output, path) !== "exports.d.ts",
    )) {
      const target = join(mergedDir, relative(output, candidate))
      if (existsSync(target) && readFileSync(target, "utf8") !== readFileSync(candidate, "utf8")) {
        throw new Error(`Declaration differs between worlds: ${relative(output, candidate)}`)
      }
      if (!existsSync(target)) {
        mkdirSync(dirname(target), { recursive: true })
        cpSync(candidate, target)
      }
    }
    cpSync(join(output, "exports.d.ts"), join(mergedDir, template.declarationFile))
  }
  const custom = join(packageDir, "golem-types", "node-sqlite-extensions.d.ts")
  if (existsSync(custom)) cpSync(custom, join(mergedDir, "node-sqlite-extensions.d.ts"))
  rmSync(typesDir, { recursive: true, force: true })
  renameSync(mergedDir, typesDir)
} finally {
  rmSync(temporaryDir, { recursive: true, force: true })
}
