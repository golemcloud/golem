import { afterEach, expect, test } from "bun:test"
import * as fs from "node:fs/promises"
import { dirname, join, resolve } from "node:path"
import { pathToFileURL } from "node:url"
import { parse, stringify } from "smol-toml"
import { digest } from "./tooling-io.ts"

const root = resolve(import.meta.dir, "../.."), cargo = Bun.which("cargo")!, runner = Bun.which("cargo-test-r")!
const tasks = parse(await fs.readFile(join(root, "Makefile.main-validation.toml"), "utf8")).tasks as Record<
  string,
  Record<string, unknown>
>
const directories: string[] = []
async function command(args: string[], env = process.env, cwd = root) {
  const child = Bun.spawn(args, { cwd, env, stdout: "pipe", stderr: "pipe" })
  const [code, stdout, stderr] = await Promise.all([
    child.exited,
    new Response(child.stdout).text(),
    new Response(child.stderr).text()
  ])
  return { code, stdout, stderr }
}
async function native(args: string[]) {
  const result = await command([runner, ...args, "--deadline", "10m", "--kill-grace", "10s"])
  if (result.code !== 0) throw new Error(result.stderr)
  return result.stdout
}
async function fixture() {
  const parent = join(root, ".amp/in/orb-backup/snapshot-tests")
  await fs.mkdir(parent, { recursive: true })
  const directory = await fs.mkdtemp(join(parent, "case-"))
  directories.push(directory)
  const target = join(directory, "target"), reports = join(directory, "reports"), input = join(directory, "input.json")
  const destination = join(target, "main-validation/frozen-cycle"), output = join(reports, "executor-selection.json")
  for (const path of [target, reports, join(directory, "bin"), join(directory, "src")]) await fs.mkdir(path)
  await fs.writeFile(join(directory, "Cargo.toml"), "[package]\nname=\"fixture\"\nversion=\"0.0.0\"\n[workspace]\n")
  await fs.writeFile(join(directory, "src/lib.rs"), "")
  await fs.writeFile(join(reports, "temporary-root.txt"), join(directory, "cycle"))
  await fs.writeFile(
    join(directory, "Makefile.toml"),
    stringify({
      tasks: {
        "main-validation-executor-native": tasks["main-validation-executor-native"]!,
        fixture: { run_task: "main-validation-executor-native" }
      }
    })
  )
  await fs.writeFile(
    join(directory, "bin/cargo"),
    `#!/bin/sh\ncase "$1" in\nbuild) exit 0;;\nnextest) cat "$SNAPSHOT_FIXTURE/input.json"; test "$SNAPSHOT_FAIL_AFTER" != metadata;;\n*) exec '${cargo}' "$@";;\nesac\n`,
    { mode: 0o755 }
  )
  await fs.writeFile(
    join(directory, "bin/cargo-test-r"),
    `#!/bin/sh\nprintf '%s\\n' "$*" >> "$SNAPSHOT_FIXTURE/runner-calls"\n'${runner}' "$@"\ncode=$?\nif [ "$SNAPSHOT_FAIL_AFTER" = "$1" ]; then exit 7; fi\nexit "$code"\n`,
    { mode: 0o755 }
  )
  const env = Object.fromEntries(
    Object.entries(process.env).filter(([key]) => !/^(CARGO_MAKE_|MAIN_VALIDATION_|TEST_R_|CARGO_TARGET_DIR)/.test(key))
  )
  Object.assign(env, {
    PATH: `${directory}/bin:${process.env.PATH}`,
    SNAPSHOT_FIXTURE: directory,
    CARGO_TARGET_DIR: target,
    MAIN_VALIDATION_REPORT_DIR: reports
  })
  const metadata = async (paths: string[]) => {
    await fs.writeFile(
      input,
      JSON.stringify({
        "rust-build-meta": {
          "target-directory": target,
          "base-output-directories": ["dev-ci"],
          "linked-paths": [],
          "non-test-binaries": {}
        },
        "rust-binaries": Object.fromEntries(paths.map((path, index) => [`fixture::test-${index}`, {
          "binary-id": `fixture::test-${index}`,
          "binary-name": `test-${index}`,
          "package-id": `path+${pathToFileURL(directory)}#fixture@0.0.0`,
          kind: "test",
          "binary-path": path,
          "build-platform": "target"
        }]))
      })
    )
  }
  const binary = async (name = "source/program") => {
    const path = join(target, name)
    await fs.mkdir(dirname(path), { recursive: true })
    await fs.copyFile("/usr/bin/true", path)
    await fs.chmod(path, 0o751)
    return path
  }
  const freeze = async (failAfter = "") => {
    const result = await command(
      [cargo, "make", "--disable-check-for-updates", "--no-workspace", "fixture"],
      { ...env, SNAPSHOT_FAIL_AFTER: failAfter },
      directory
    )
    if (result.code !== 0) throw new Error(result.stdout + result.stderr)
  }
  const verify = async () =>
    native([
      "verify",
      "--selection",
      join(destination, "selection.json"),
      "--expect-digest",
      await fs.readFile(join(reports, "executor-selection.digest"), "utf8")
    ])
  return { directory, target, input, reports, output, destination, metadata, binary, freeze, verify }
}
afterEach(async () => {
  for (const directory of directories.splice(0)) await fs.rm(directory, { recursive: true, force: true })
})

async function compile(output: string, source: string, flags: string[] = []): Promise<string> {
  await fs.mkdir(dirname(output), { recursive: true })
  const child = Bun.spawn(["cc", "-x", "c", "-", "-o", output, ...flags], {
    stdin: "pipe",
    stdout: "pipe",
    stderr: "pipe"
  })
  child.stdin.write(source)
  child.stdin.end()
  const stderr = await new Response(child.stderr).text()
  expect(await child.exited, stderr).toBe(0)
  return output
}
const library = (directory: string) =>
  compile(join(directory, "libsnapshot_fixture.so"), "int fixture(void) { return 0; }", ["-shared", "-fPIC"])
const linked = (directory: string, lib: string, rpath: string) =>
  compile(join(directory, "program"), "extern int fixture(void); int main(void) { return fixture(); }", [
    `-L${dirname(lib)}`,
    "-lsnapshot_fixture",
    `-Wl,-rpath,${rpath}`
  ])

test("production producer freezes every executable, pins native evidence and exports Lima checksums", async () => {
  const f = await fixture(),
    sources = await Promise.all(Array.from({ length: 3 }, (_, i) => f.binary(`source-${i}/same-name`)))
  await f.metadata(sources)
  await f.freeze()
  const selection = await Bun.file(f.output).json() as { binaries: { executable: string }[] }
  expect(selection.binaries).toHaveLength(3)
  for (const [index, binary] of selection.binaries.entries()) {
    expect((await fs.stat(binary.executable)).ino).not.toBe((await fs.stat(sources[index]!)).ino)
    expect((await fs.stat(binary.executable)).mode & 0o777).toBe(0o751)
    expect(await digest(binary.executable)).toBe(await digest(sources[index]!))
  }
  expect(await fs.readFile(join(f.reports, "executor-selection.digest"), "utf8")).toBe(
    "sha256:" + await digest(f.output)
  )
  expect((await command(["sha256sum", "--check", "--strict", join(f.reports, "executor-executables.sha256")])).code)
    .toBe(0)
  const calls = (await fs.readFile(join(f.directory, "runner-calls"), "utf8")).trim().split("\n")
  expect(calls.map((line) => line.split(" ")[0])).toEqual(["list", "snapshot", "verify"])
  for (const call of calls) expect(call).toContain("--deadline 10m --kill-grace 10s")
  await fs.writeFile(sources[0]!, "compiler overwrite")
  await fs.unlink(sources[1]!)
  await f.verify()
  // Production package/target filters now address the native snapshot directly.
  expect(
    (await command([
      runner,
      "run",
      "--selection",
      join(f.destination, "selection.json"),
      "--expect-digest",
      "sha256:" + await digest(f.output),
      "--package",
      "fixture",
      "--test",
      "test-0"
    ])).code
  ).toBe(0)
  await fs.rm(f.destination, { recursive: true })
  expect(await fs.exists(f.output)).toBe(true)
  expect(await fs.exists(join(f.reports, "executor-selection.digest"))).toBe(true)
})

test("failed production selection or snapshot clears stale admission evidence", async () => {
  for (
    const kind of [
      "missing",
      "empty",
      "non-executable",
      "text",
      "truncated",
      "existing-destination",
      "metadata",
      "list",
      "snapshot",
      "verify"
    ]
  ) {
    const f = await fixture(), source = await f.binary()
    await f.metadata(kind === "empty" ? [] : [source, ...(kind === "missing" ? [join(f.target, "missing")] : [])])
    await fs.writeFile(join(f.reports, "executor-selection.digest"), "stale-success")
    if (kind === "non-executable") await fs.chmod(source, 0o644)
    if (kind === "text") await fs.writeFile(source, "#!/bin/sh\nexit 0\n")
    if (kind === "truncated") await fs.writeFile(source, (await fs.readFile(source)).subarray(0, 64))
    if (kind === "existing-destination") await fs.mkdir(f.destination, { recursive: true })
    await expect(f.freeze(kind)).rejects.toThrow()
    expect(await fs.exists(join(f.reports, "executor-selection.digest"))).toBe(false)
    expect(await fs.exists(f.output)).toBe(false)
  }
})

test("real static ELF is accepted by production snapshotting", async () => {
  const f = await fixture()
  await f.metadata([await compile(join(f.target, "static"), "int main(void) { return 0; }", ["-static"])])
  await f.freeze()
  await f.verify()
})

test("native preparation budgets permit healthy pool inspection exceeding one minute", async () => {
  const f = await fixture(), loader = join(f.directory, "slow-loader")
  const interpreter = (await Bun.file("/proc/self/maps").text()).split("\n")
    .map((line) => line.trim().split(/\s+/).at(-1)!).find((path) => /\/ld-linux[^/]*\.so/.test(path))
  expect(interpreter).toBeDefined()
  await fs.writeFile(loader, `#!/bin/sh\nsleep 31\nexec '${interpreter}' "$@"\n`, { mode: 0o755 })
  await f.metadata([
    await compile(join(f.target, "slow"), "int main(void) { return 0; }", [`-Wl,--dynamic-linker,${loader}`])
  ])
  const started = performance.now()
  await f.freeze()
  expect(performance.now() - started).toBeGreaterThan(60000)
  await f.verify()
}, 90000)

test("native preparation deadline drains TERM-resistant loaders and removes unpublished staging", async () => {
  const f = await fixture(), loader = join(f.directory, "loader"), marker = join(f.directory, "loader.pid")
  await fs.writeFile(loader, `#!/bin/sh\ntrap '' TERM\necho $$ > '${marker}'\nsleep 60\n`, { mode: 0o755 })
  await f.metadata([
    await compile(join(f.target, "blocked"), "int main(void) { return 0; }", [`-Wl,--dynamic-linker,${loader}`])
  ])
  const selection = join(f.directory, "selection.json")
  await fs.writeFile(selection, await native(["list", "--binaries-metadata", f.input]))
  const result = await command([
    runner,
    "snapshot",
    "--selection",
    selection,
    "--out",
    join(f.directory, "blocked-snapshot"),
    "--deadline",
    "500ms",
    "--kill-grace",
    "100ms"
  ])
  expect(result.code, result.stderr).toBe(124)
  const pid = Number(await fs.readFile(marker, "utf8")),
    stat = await fs.readFile(`/proc/${pid}/stat`, "utf8").catch(() => "")
  expect(stat === "" || stat.slice(stat.lastIndexOf(")") + 2).startsWith("Z")).toBe(true)
  expect((await fs.readdir(f.directory)).filter((name) => name.includes("blocked-snapshot"))).toEqual([])
})

test("native loader policy rejects unresolved, mutable and symlinked Cargo libraries", async () => {
  for (const kind of ["unresolved", "managed", "symlink"]) {
    const f = await fixture(), lib = await library(join(kind === "unresolved" ? f.directory : f.target, "lib"))
    let search = dirname(lib)
    if (kind === "symlink") {
      search = join(f.directory, "external")
      await fs.mkdir(search)
      await fs.symlink(lib, join(search, "libsnapshot_fixture.so"))
    }
    const source = await linked(join(f.directory, "runtime"), lib, search)
    if (kind === "unresolved") await fs.unlink(lib)
    await f.metadata([source])
    await expect(f.freeze()).rejects.toThrow()
    expect(await fs.exists(f.output)).toBe(false)
  }
})

test("external libraries survive native relocation but ORIGIN changes remain rejected", async () => {
  for (const kind of ["external", "origin-missing", "origin-fallback"]) {
    const f = await fixture(), lib = await library(join(f.directory, "runtime/source"))
    const fallback = kind === "origin-fallback" ? await library(join(f.directory, "fallback")) : undefined
    await f.metadata([
      await linked(
        dirname(lib),
        lib,
        kind === "external" ? dirname(lib) : `$ORIGIN${fallback ? ":" + dirname(fallback) : ""}`
      )
    ])
    if (kind === "external") {
      await f.freeze()
      await f.verify()
    } else {
      await expect(f.freeze()).rejects.toThrow()
      expect(await fs.exists(f.output)).toBe(false)
    }
  }
})

test("native filtered runs still reject excluded binary, mode and manifest tampering", async () => {
  const f = await fixture()
  await f.metadata([await f.binary("first"), await f.binary("second")])
  await f.freeze()
  const path = join(f.destination, "selection.json"), original = await fs.readFile(path)
  const selection = await Bun.file(path).json() as { binaries: { executable: string }[] }
  const excluded = selection.binaries[1]!.executable, bytes = await fs.readFile(excluded)
  for (const kind of ["bytes", "mode", "selection", "missing"]) {
    if (kind === "bytes") await fs.writeFile(excluded, "tampered")
    if (kind === "mode") await fs.chmod(excluded, 0o700)
    if (kind === "selection") await fs.appendFile(path, " ")
    if (kind === "missing") await fs.unlink(excluded)
    await expect(f.verify()).rejects.toThrow()
    const result = await command([
      runner,
      "run",
      "--selection",
      path,
      "--expect-digest",
      "sha256:" + await digest(f.output),
      "--package",
      "fixture",
      "--test",
      "test-0"
    ])
    expect(result.code, result.stderr).not.toBe(0)
    await fs.writeFile(excluded, bytes)
    await fs.chmod(excluded, 0o751)
    await fs.writeFile(path, original)
  }
  const result = await command([runner, "verify", "--selection", path], { ...process.env, PATH: "/no-tools" })
  expect(result.code, result.stderr).toBe(0)
})
