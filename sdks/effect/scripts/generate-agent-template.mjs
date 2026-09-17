import { spawnSync } from "node:child_process"
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { templateMatrix } from "./template-matrix.mjs"

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const sourceWit = join(packageDir, "wit")
const witBindgenLine =
  'wit-bindgen-p3 = { package = "wit-bindgen", version = "0.58.0", default-features = false, features = ["async", "async-spawn", "macros", "inter-task-wakeup"], optional = true }'
const forkedLine =
  'wit-bindgen-p3 = { package = "wit-bindgen", git = "https://github.com/golemcloud/wit-bindgen", rev = "4407232ead86d9bcbd06cbebd790a52120a4087a", version = "=0.59.0", default-features = false, features = ["async", "async-spawn", "macros", "inter-task-wakeup"], optional = true }'

const sharedModules = [
  ["@golemcloud/effect-golem/sqlite", "dist/sqlite.mjs"],
  ["@golemcloud/effect-golem/postgres", "dist/postgres.mjs"],
  ["@golemcloud/effect-golem/mysql", "dist/mysql.mjs"],
  ["@golemcloud/effect-golem/ignite2", "dist/ignite.mjs"],
  ["effect", "dist/effect.mjs"],
]

for (const template of templateMatrix) {
  const modules = [
    [template.sdkModuleName, template.sdkEntry],
    ...(template.role === "tool-middleware" ? [["effect", "dist/effect.mjs"]] : sharedModules),
  ]
  for (const [, entry] of modules) {
    if (!existsSync(join(packageDir, entry))) {
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
  args.push("--js-modules", "user=@slot")

  const result = spawnSync("wasm-rquickjs", args, { cwd: packageDir, stdio: "inherit" })
  if (result.error) throw result.error
  if (result.status !== 0) process.exit(result.status ?? 1)

  const cargoToml = join(output, "Cargo.toml")
  const original = readFileSync(cargoToml, "utf8")
  const count = original.split(witBindgenLine).length - 1
  if (count !== 1)
    throw new Error(`Expected one pinned wit-bindgen line in ${cargoToml}, found ${count}`)
  writeFileSync(cargoToml, original.replace(witBindgenLine, forkedLine))
  rmSync(join(output, "Cargo.lock"), { force: true })
}
