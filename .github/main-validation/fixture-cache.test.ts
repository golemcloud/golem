import { afterEach, beforeEach, expect, spyOn, test } from "bun:test"
import { Effect, Fiber } from "effect"
import * as fs from "node:fs/promises"
import { dirname, join, resolve } from "node:path"
import {
  command,
  fingerprint,
  identity,
  publish,
  restore,
  run,
  sourceInputs,
  stage,
  stamps,
  verify
} from "./fixture-cache.ts"

const repository = resolve(import.meta.dir, "../..")
const scratch = join(repository, ".amp/in/orb-backup/fixture-cache-tests")
const execute = Effect.runPromise
let root: string, cache: string, pending: string
const write = async (path: string, value: string) => {
  const target = join(root, path)
  await fs.mkdir(dirname(target), { recursive: true })
  await fs.writeFile(target, value)
  return target
}
const track = () => execute(command(["git", "add", "."], root))
const key = (app = "example", variant = "release") => execute(fingerprint(root, app, variant))
const bundle = async () => {
  const k = await key(), before = await execute(stamps(root))
  await write("test-components/first.wasm", "first compiled output")
  await fs.chmod(await write("test-components/second.wasm", "second compiled output"), 0o751)
  await execute(stage(root, pending, k, before))
  return k
}
beforeEach(async () => {
  await fs.mkdir(scratch, { recursive: true })
  root = await fs.mkdtemp(join(scratch, "case-"))
  cache = join(root, "cache/completed")
  pending = join(root, "cache/pending")
  await fs.mkdir(pending, { recursive: true })
  await write("Cargo.toml", "[workspace]\n[workspace.dependencies]\n")
  await write("cli/golem-cli/Cargo.toml", "[package]\nname=\"fixture-cli\"\nversion=\"1.0.0\"\n")
  for (
    const path of [
      "cli/golem-cli/src/lib.rs",
      "sdks/anything/runtime",
      "wit/interface.wit",
      "test-components/example/golem.yaml",
      "test-components/other/source",
      "test-components/shared.yaml"
    ]
  ) await write(path, path)
  await execute(command(["git", "init", "-q"], root))
  await track()
})
afterEach(async () => {
  await fs.rm(root, { recursive: true, force: true })
})

test("interrupting a cache command joins its acquired process", async () => {
  const ready = join(root, "child-pid")
  const fiber = Effect.runFork(command([
    process.execPath,
    "-e",
    `await Bun.write(${JSON.stringify(ready)}, String(process.pid)); await Bun.sleep(30000)`
  ], root))
  let pid: number | undefined
  try {
    const deadline = Date.now() + 10000
    while (!(await fs.exists(ready))) {
      if (Date.now() > deadline) throw new Error("cache command did not start")
      await Bun.sleep(10)
    }
    const childPid = Number(await fs.readFile(ready, "utf8"))
    pid = childPid
    await execute(Fiber.interrupt(fiber))
    expect(() => process.kill(childPid, 0)).toThrow()
  } finally {
    await execute(Fiber.interrupt(fiber))
    if (pid !== undefined) {
      try {
        process.kill(pid, "SIGKILL")
      } catch { /* Already joined. */ }
    }
  }
})

test("arbitrary multifile bundles restore only exports and modes, replacing stale links", async () => {
  const k = await bundle()
  await execute(publish(pending, cache))
  const outside = await write("outside", "untouched")
  await fs.rm(join(root, "test-components/first.wasm"))
  await fs.symlink(outside, join(root, "test-components/first.wasm"))
  await execute(restore(root, join(cache, k), k))
  expect(await fs.readFile(outside, "utf8")).toBe("untouched")
  expect(await fs.readFile(join(root, "test-components/first.wasm"), "utf8")).toBe("first compiled output")
  expect((await fs.stat(join(root, "test-components/second.wasm"))).mode & 0o777).toBe(0o751)
  expect(Object.keys(await execute(verify(join(cache, k), k))).sort()).toEqual(["first.wasm", "second.wasm"])
})
test("eviction retains current restores and builds, including hit-only cycles", async () => {
  const restored = await bundle()
  await write("test-components/example/golem.yaml", "obsolete")
  const obsolete = await bundle()
  await execute(publish(pending, cache))
  await execute(run(root, cache, "begin"))
  await write("cache/pending/restored.json", JSON.stringify({ key: restored }))
  await write("test-components/example/golem.yaml", "new")
  const built = await bundle()
  await execute(publish(pending, cache))
  expect((await fs.readdir(cache)).sort()).toEqual([restored, built].sort())
  expect(await fs.exists(join(cache, obsolete))).toBe(false)
  await execute(run(root, cache, "begin"))
  await write("cache/pending/restored.json", JSON.stringify({ key: restored }))
  await execute(publish(pending, cache))
  expect(await fs.readdir(cache)).toEqual([restored])
})
test("failed publication never evicts previous entries", async () => {
  const previous = await bundle()
  await execute(publish(pending, cache))
  await execute(run(root, cache, "begin"))
  await write("test-components/example/golem.yaml", "new")
  const broken = await bundle()
  await fs.rm(join(pending, broken, "files/first.wasm"))
  await expect(execute(publish(pending, cache))).rejects.toThrow()
  await execute(restore(root, join(cache, previous), previous))
  expect(await fs.readdir(cache)).toEqual([previous])
})
test("source invalidation is per application and language independent; skills remain conservative inputs", async () => {
  await write("golem-skills/skills/common/example/SKILL.md", "skill text")
  await track()
  const baseline = await key()
  for (
    const path of [
      "sdks/anything/runtime",
      "cli/golem-cli/src/lib.rs",
      "wit/interface.wit",
      "test-components/example/golem.yaml",
      "test-components/shared.yaml",
      "golem-skills/skills/common/example/SKILL.md"
    ]
  ) {
    const original = await fs.readFile(join(root, path), "utf8")
    await write(path, "changed")
    expect(await key()).not.toBe(baseline)
    await write(path, original)
  }
  await write("test-components/other/source", "changed sibling")
  await write("golem-worker-executor/src/lib.rs", "unrelated")
  await track()
  expect(await key()).toBe(baseline)
  expect(await key("example", "quick")).not.toBe(baseline)
  for (const language of ["rust", "ts", "effect", "scala", "moonbit", "go"]) {
    await write(`test-components/new-${language}/source`, language)
    await track()
    expect(await key(`new-${language}`)).toMatch(/^[0-9a-f]{64}$/)
  }
})
test("added, deleted, and mode changed source invalidates", async () => {
  const baseline = await key()
  const file = await write("test-components/example/new", "new")
  await track()
  const added = await key()
  expect(added).not.toBe(baseline)
  await fs.chmod(file, 0o755)
  expect(await key()).not.toBe(added)
  await fs.rm(file)
  await execute(command(["git", "rm", "--cached", file], root))
  expect(await key()).toBe(baseline)
})
test("transitive CLI workspace/target/build dependencies invalidate but independent originals do not", async () => {
  await write("Cargo.toml", "[workspace]\n[workspace.dependencies]\nshared={path=\"shared\"}\nregistry=\"1.0\"\n")
  await write(
    "cli/golem-cli/Cargo.toml",
    "[dependencies]\nregistry={workspace=true}\n[target.'cfg(unix)'.build-dependencies]\nshared={workspace=true}\n"
  )
  await write("shared/Cargo.toml", "[package]\nname=\"shared\"\nversion=\"1.0.0\"\n")
  await write("shared/src/lib.rs", "shared")
  await write("test-components/example/local-provider/source", "local")
  await write("builtin-tools/example/source", "original")
  await track()
  const baseline = await key()
  await write("shared/src/lib.rs", "changed")
  expect(await key()).not.toBe(baseline)
  await write("shared/src/lib.rs", "shared")
  await write("builtin-tools/example/source", "changed original")
  expect(await key()).toBe(baseline)
  await write("test-components/example/local-provider/source", "changed local")
  expect(await key()).not.toBe(baseline)
})
test("stage rejects all mutated inputs; repeated unchanged builds retain input identity", async () => {
  for (
    const paths of [["test-components/example/golem.yaml"], [
      "sdks/anything/runtime",
      "wit/interface.wit",
      "cli/golem-cli/src/lib.rs"
    ]]
  ) {
    await execute(run(root, cache, "begin"))
    expect(await execute(run(root, cache, "restore", "example"))).toBe(1)
    for (const path of paths) await write(path, "mutated input")
    await write("test-components/export.wasm", "compiled")
    expect(await execute(run(root, cache, "stage", "example"))).toBe(0)
    expect((await fs.readdir(pending)).filter((p) => !p.endsWith(".json"))).toEqual([])
  }
  const baseline = await execute(sourceInputs(root, "example"))
  for (let i = 0; i < 2; i++) {
    await execute(run(root, cache, "begin"))
    await execute(run(root, cache, "restore", "example"))
    await write("test-components/export.wasm", `compiled ${i}`)
    await execute(run(root, cache, "stage", "example"))
    await execute(verify(join(pending, await key()), await key()))
    expect(await execute(sourceInputs(root, "example"))).toEqual(baseline)
  }
})
test("corrupt, missing, extra, empty, unsafe, symlink, malformed bundles fail before copying", async () => {
  const k = await bundle(), b = join(pending, k), payload = join(b, "files"), marker = join(b, "COMPLETE.json")
  for (
    const defect of [
      "corrupt",
      "missing",
      "extra",
      "empty",
      "unsafe",
      "symlink",
      "malformed",
      "directory",
      "marker-link",
      "bundle-link"
    ]
  ) {
    await fs.rm(b, { recursive: true, force: true })
    await execute(stage(root, pending, k, {}))
    if (defect === "corrupt") await fs.writeFile(join(payload, "first.wasm"), "bad")
    if (defect === "missing") await fs.rm(join(payload, "second.wasm"))
    if (defect === "extra") await fs.writeFile(join(payload, "extra.wasm"), "extra")
    if (defect === "empty" || defect === "unsafe") {
      const m = JSON.parse(await fs.readFile(marker, "utf8"))
      m.files = defect === "empty" ? {} : { "../escape.wasm": { size: 1 } }
      await fs.writeFile(marker, JSON.stringify(m))
    }
    if (defect === "symlink") {
      await fs.rm(join(payload, "first.wasm"))
      await fs.symlink(join(root, "test-components/first.wasm"), join(payload, "first.wasm"))
    }
    if (defect === "malformed") await fs.writeFile(marker, "null")
    if (defect === "directory") await fs.mkdir(join(payload, "extra.wasm"))
    if (defect === "marker-link") {
      await fs.rename(marker, join(root, "marker"))
      await fs.symlink(join(root, "marker"), marker)
    }
    if (defect === "bundle-link") {
      await fs.rename(b, join(root, "bundle"))
      await fs.symlink(join(root, "bundle"), b)
    }
    await expect(execute(restore(root, b, k))).rejects.toThrow()
    expect(await fs.readFile(join(root, "test-components/first.wasm"), "utf8")).toBe("first compiled output")
  }
})
test("empty exports and export symlinks cannot stage", async () => {
  const k = await key()
  await expect(execute(stage(root, pending, k, await execute(stamps(root))))).rejects.toThrow()
  await write("test-components/empty.wasm", "")
  await expect(execute(stage(root, pending, k, {}))).rejects.toThrow()
  await fs.rm(join(root, "test-components/empty.wasm"))
  await fs.symlink(await write("outside", "output"), join(root, "test-components/link.wasm"))
  await expect(execute(stage(root, pending, k, {}))).rejects.toThrow()
  expect(await fs.exists(join(pending, k))).toBe(false)
})
test("source links hash checked-in targets, not generated build trees; escapes are refused", async () => {
  await write("shared-source/implementation", "first")
  await fs.symlink("../../shared-source/implementation", join(root, "test-components/example/source"))
  await fs.symlink("_build", join(root, "sdks/anything/target"))
  await track()
  const baseline = await key()
  await write("shared-source/implementation", "second")
  expect(await key()).not.toBe(baseline)
  await write("shared-source/implementation", "first")
  await write("sdks/anything/_build/generated", "output")
  expect(await key()).toBe(baseline)
  await fs.symlink("/etc/passwd", join(root, "test-components/example/escape"))
  await track()
  await expect(key()).rejects.toThrow("outside checkout")
})
test("dangling symlink chains cannot escape the checkout", async () => {
  await fs.symlink("/nonexistent-outside-checkout", join(root, "indirect"))
  await fs.symlink("../../indirect/missing", join(root, "test-components/example/source"))
  await track()
  await expect(key()).rejects.toThrow("outside checkout")
})
test("missing and malformed receipts remain best-effort stage misses", async () => {
  const k = await key()
  expect(await execute(run(root, cache, "stage", "example"))).toBe(0)
  for (
    const value of [
      "null",
      "{",
      JSON.stringify({ key: k, before: null }),
      JSON.stringify({ key: "different", sources: null }),
      JSON.stringify({ key: k, before: { "export.wasm": 1 }, sources: {} }),
      JSON.stringify({ key: k, before: {}, sources: { "source": ["digest", "not a mode"] } })
    ]
  ) {
    await write(`cache/pending/${identity(["example", "release"])}.json`, value)
    expect(await execute(run(root, cache, "stage", "example"))).toBe(0)
    expect(await fs.exists(join(pending, k))).toBe(false)
  }
})
test("inventory validation rejects malformed and extra entry fields without stripping them", async () => {
  const k = await bundle(), marker = join(pending, k, "COMPLETE.json")
  const original = JSON.parse(await fs.readFile(marker, "utf8"))
  for (const fields of [{ size: "1" }, { mode: null }, { extra: "secret-marker" }]) {
    await fs.writeFile(
      marker,
      JSON.stringify({
        ...original,
        files: { ...original.files, "first.wasm": { ...original.files["first.wasm"], ...fields } }
      })
    )
    await expect(execute(verify(join(pending, k), k))).rejects.toThrow()
  }
  expect((await fs.readdir(pending)).some((name) => name.startsWith(".stage-"))).toBe(false)
})
test("diagnostics expose input identity and changed paths but not parser contents", async () => {
  const log = spyOn(console, "log").mockImplementation(() => {})
  try {
    await execute(run(root, cache, "begin"))
    await execute(run(root, cache, "restore", "example"))
    await write("test-components/example/golem.yaml", "fixture-secret-marker")
    await execute(run(root, cache, "stage", "example"))
    const diagnostics = log.mock.calls.flat().join("\n")
    expect(diagnostics).toContain("source-digest=")
    expect(diagnostics).toContain("Fixture cache input groups:")
    expect(diagnostics).toContain("test-components/example/golem.yaml")
    expect(diagnostics).not.toContain("fixture-secret-marker")
    log.mockClear()
    await write("Cargo.toml", "secret = \"fixture-secret-marker\n")
    expect(await execute(run(root, cache, "restore", "example"))).toBe(1)
    expect(log.mock.calls.flat().join("\n")).not.toContain("fixture-secret-marker")
  } finally {
    log.mockRestore()
  }
})

test("real shell cold/warm/check cycles cover every language, benchmarks and failure", async () => {
  const script = await fs.readFile(join(repository, "test-components/build-components.sh"), "utf8")
  const declarations = [...script.matchAll(/^\w+_apps=\((.*)\)$/gm)].map((m) =>
    [...m[1]!.matchAll(/"([^"]+)"/g)].map((v) => v[1]!)
  )
  for (const app of declarations.flat()) await write(`test-components/${app}/golem.yaml`, `app: ${app}`)
  for (
    const directory of [
      "tool-streaming/rust-provider",
      "tool-streaming/rust-caller",
      "agent-rpc/golem-it-agent-rpc-rust",
      "size-analysis/fixtures"
    ]
  ) await write(`test-components/${directory}/Cargo.toml`, "[package]\n")
  await write("test-components/build-components.sh", script)
  await write(
    ".github/main-validation/fixture-cache.ts",
    await fs.readFile(join(import.meta.dir, "fixture-cache.ts"), "utf8")
  )
  await fs.symlink(join(import.meta.dir, "node_modules"), join(root, ".github/main-validation/node_modules"))
  const cli = await write(
    "bin/golem",
    `#!/bin/bash
set -eu
echo "\${PWD##*/}:$*" >> "$EVENTS"
if [[ "$*" == *"--skip-check"* && "\${FAIL_APP:-}" == "\${PWD##*/}" ]]; then exit 7; fi
if [[ "$*" == *"--skip-check"* ]]; then
 mkdir -p "$CARGO_MAKE_CRATE_TARGET_DIRECTORY/wasm32-wasip2/release"
 for name in golem_it_tool_streaming_rust_provider golem_it_tool_streaming_rust_caller golem_it_agent_rpc_rust; do printf 'benchmark:%s' "$name" > "$CARGO_MAKE_CRATE_TARGET_DIRECTORY/wasm32-wasip2/release/$name.wasm"; done
fi
if [[ "$*" == *"exec copy"* ]]; then
 printf 'compiled:%s' "\${PWD##*/}" > "../\${PWD##*/}.wasm"
 case "\${PWD##*/}" in *scala*|*moonbit*) name="golem_it_\${PWD##*/}"; printf compiled > "../\${name//-/_}.wasm" ;; esac
fi
`
  )
  await fs.chmod(cli, 0o755)
  await fs.chmod(await write("bin/npm", "#!/bin/bash\nexit 0\n"), 0o755)
  await fs.chmod(
    await write(
      "bin/cargo",
      `#!/bin/bash
set -eu
if [ "$1" = metadata ]; then printf '{"target_directory":"%s"}' "$CARGO_MAKE_CRATE_TARGET_DIRECTORY"; fi
if [ "$1" = build ]; then
 mkdir -p "$CARGO_MAKE_CRATE_TARGET_DIRECTORY/wasm32-wasip2/release"
 for name in retention_canonical_floor retention_empty retention_non_reflective retention_reflection; do printf 'retention:%s' "$name" > "$CARGO_MAKE_CRATE_TARGET_DIRECTORY/wasm32-wasip2/release/$name.wasm"; done
fi
`
    ),
    0o755
  )
  await fs.symlink(process.execPath, join(root, "bin/bun"))
  await track()
  const events = join(root, "events"),
    env: NodeJS.ProcessEnv = {
      ...process.env,
      GOLEM_CLI: cli,
      GOLEM_FIXTURE_CACHE: cache,
      CARGO_MAKE_CRATE_TARGET_DIRECTORY: join(root, "target"),
      CARGO_TARGET_DIR: "",
      PATH: `${join(root, "bin")}:${process.env.PATH}`,
      EVENTS: events
    }
  const shell = (...args: string[]) =>
    execute(command(["bash", "test-components/build-components.sh", ...args], root, env, 180_000))
  const exports = async () => {
    const files: Record<string, string> = {}
    for (const name of await fs.readdir(join(root, "test-components"))) {
      if (name.endsWith(".wasm")) files[name] = await fs.readFile(join(root, "test-components", name), "utf8")
    }
    return files
  }
  await execute(run(root, cache, "begin"))
  await shell("rebuild")
  await shell("benchmarks", "rebuild")
  const cold = await exports()
  expect(Object.keys(cold).length).toBeGreaterThan(30)
  for (
    const name of ["retention_canonical_floor", "retention_empty", "retention_non_reflective", "retention_reflection"]
  ) {
    expect(cold[`${name}.wasm`]).toBe(`retention:${name}`)
  }
  await execute(publish(pending, cache))
  for (const name of Object.keys(cold)) await fs.rm(join(root, "test-components", name))
  await fs.rm(events)
  await execute(run(root, cache, "begin"))
  expect(await shell("rebuild")).toContain("Fixture cache hit:")
  await shell("benchmarks", "rebuild")
  expect(await fs.exists(events)).toBe(false)
  expect(await exports()).toEqual(cold)
  await shell("check")
  await shell("benchmarks", "check")
  const lines = (await fs.readFile(events, "utf8")).trim().split("\n")
  expect(lines.length).toBe(declarations.flat().length + 3)
  expect(lines.every((line) => line.includes("--step check"))).toBe(true)
  await write("test-components/agent-sdk-go/golem.yaml", "changed")
  env.FAIL_APP = "agent-sdk-go"
  await expect(shell("go", "rebuild")).rejects.toThrow("(7)")
}, 180000)
