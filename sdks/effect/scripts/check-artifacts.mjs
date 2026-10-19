import { existsSync, readFileSync } from "node:fs"
import { currentProvenance, manifestPath } from "./artifact-manifest.mjs"

let recorded
try {
  recorded = JSON.parse(readFileSync(manifestPath, "utf8"))
} catch {
  recorded = null
}
const current = currentProvenance()
if (!existsSync(manifestPath) || JSON.stringify(recorded) !== JSON.stringify(current)) {
  console.error(
    "Missing, modified, or stale Effect template artifacts; run npm run build-agent-template.",
  )
  process.exit(1)
}
