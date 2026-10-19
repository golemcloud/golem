import { readFileSync, readdirSync, statSync } from "node:fs"
import { dirname, join, relative, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const roots = ["src", "wit/main.wit", "rollup.config.mjs", "tsconfig.json", "vitest.config.ts"]
const stale =
  /golem:(?:agent|core)(?:\/[a-z-]+)?@1\.5\.0|golem:durability\/durability@1\.5\.0|wasi:(?:cli|clocks|io)\/[a-z-]+@0\.2\.3/g

function files(path) {
  if (!statSync(path).isDirectory()) return [path]
  return readdirSync(path).flatMap((entry) => files(join(path, entry)))
}

const offenders = roots
  .flatMap((root) => files(join(packageDir, root)))
  .flatMap((path) => {
    const matches = [...readFileSync(path, "utf8").matchAll(stale)]
    return matches.map((match) => `${relative(packageDir, path)}: ${match[0]}`)
  })
if (offenders.length > 0) {
  console.error(`Obsolete Golem 1.5/WASI 0.2.3 contracts remain:\n${offenders.join("\n")}`)
  process.exit(1)
}
