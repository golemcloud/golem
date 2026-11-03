import { spawnSync } from "node:child_process"
import { existsSync, rmSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { templateMatrix } from "./template-matrix.mjs"
import {
  removeSkeletonLock,
  templateGeneratorArgs,
  toolProcessModules,
} from "../../javascript/tool-process-template.mjs"

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const sourceWit = join(packageDir, "wit")
const sharedModules = [
  ["effect", "dist/effect.mjs"],
  ["effect/http", "dist/effect-http.mjs"],
  ["effect/http-api", "dist/effect-httpapi.mjs"],
  ...toolProcessModules,
]

for (const template of templateMatrix) {
  const modules = [[template.sdkModuleName, template.sdkEntry], ...sharedModules]
  for (const [, entry] of modules) {
    if (!existsSync(resolve(packageDir, entry))) {
      throw new Error(`${entry} does not exist; run npm run build:bundle first`)
    }
  }

  const output = join(packageDir, template.wrapperDirectory)
  rmSync(output, { recursive: true, force: true })
  const args = [
    "generate-wrapper-crate",
    "--wit",
    sourceWit,
    "--output",
    output,
    "--world",
    template.world,
    "--target",
    "wasi-p3",
  ]
  for (const [name, entry] of modules) args.push("--js-modules", `${name}=${entry}`)
  args.push("--js-modules", "user=@slot", ...templateGeneratorArgs)

  const result = spawnSync("wasm-rquickjs", args, { cwd: packageDir, stdio: "inherit" })
  if (result.error) throw result.error
  if (result.status !== 0) process.exit(result.status ?? 1)

  removeSkeletonLock(output)
}
