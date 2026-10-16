import { rmSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const dist = resolve(dirname(fileURLToPath(import.meta.url)), "../dist")
for (const directory of ["src", "component"])
  rmSync(resolve(dist, directory), { recursive: true, force: true })
