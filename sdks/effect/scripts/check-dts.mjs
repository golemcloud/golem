import { spawnSync } from "node:child_process"
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, relative, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const generatedDir = mkdtempSync(join(tmpdir(), "effect-golem-dts-"))
try {
  const result = spawnSync(process.execPath, [joinScript("generate-dts.mjs")], {
    cwd: packageDir,
    env: { ...process.env, GOLEM_DTS_OUTPUT: generatedDir },
    stdio: "inherit",
  })
  if (result.error) throw result.error
  if (result.status !== 0) process.exit(result.status ?? 1)

  const actualDir = join(packageDir, "golem-types")
  const actual = filesBelow(actualDir)
  const expected = filesBelow(generatedDir)
  const names = [...new Set([...actual.keys(), ...expected.keys()])].sort()
  const drift = names.filter(
    (name) =>
      !actual.has(name) || !expected.has(name) || !actual.get(name).equals(expected.get(name)),
  )
  if (drift.length > 0) {
    console.error(
      `Generated Effect WIT declarations are stale:\n${drift
        .map(
          (name) =>
            `${actual.has(name) ? (expected.has(name) ? "changed" : "unexpected") : "missing"}: ${name}`,
        )
        .join("\n")}\nRun npm run generate-dts.`,
    )
    process.exitCode = 1
  }
} finally {
  rmSync(generatedDir, { recursive: true, force: true })
}

function joinScript(name) {
  return resolve(packageDir, "scripts", name)
}

function filesBelow(root, current = root, result = new Map()) {
  for (const entry of readdirSync(current)) {
    const path = join(current, entry)
    if (statSync(path).isDirectory()) filesBelow(root, path, result)
    else result.set(relative(root, path), readFileSync(path))
  }
  return result
}
