import { afterEach, expect, test } from "bun:test"
import { Effect } from "effect"
import { watch } from "node:fs"
import * as fs from "node:fs/promises"
import { dirname, join, resolve } from "node:path"
import { freeze, loadMetadata, verify } from "./freeze-binaries.ts"
import { digest, object, uniqueJson } from "./tooling-io.ts"

const directories: string[] = []
async function fixture() {
  const parent = resolve(import.meta.dirname, "../../.amp/in/orb-backup/freeze-tests")
  await fs.mkdir(parent, { recursive: true })
  const directory = await fs.mkdtemp(join(parent, "case-"))
  directories.push(directory)
  const target = join(directory, "target"),
    input = join(directory, "input.json"),
    output = join(directory, "binaries.json"),
    destination = join(directory, "snapshot")
  await fs.mkdir(target)
  const metadata = async (paths: string[]) => {
    const result = {
      "rust-build-meta": {
        "target-directory": target,
        "linked-paths": ["native/out"],
        "non-test-binaries": { package: [{ path: "worker" }] }
      },
      "rust-binaries": Object.fromEntries(
        paths.map((
          path,
          index
        ) => [`package-${index}::generic-test`, {
          "binary-id": `package-${index}::generic-test`,
          "binary-name": "generic-test",
          "package-id": "path+file://fixture#0.0.0",
          kind: index % 2 ? "test" : "lib",
          "binary-path": path,
          "build-platform": "target",
          extra: [null, true, { untouched: 1 }]
        }])
      ),
      extra: [1, true, null]
    }
    await fs.writeFile(input, JSON.stringify(result))
    return result
  }
  const binary = async (name = "source/program") => {
    const path = join(target, name)
    await fs.mkdir(dirname(path), { recursive: true })
    await fs.copyFile("/usr/bin/true", path)
    await fs.chmod(path, 0o751)
    return path
  }
  return {
    directory,
    target,
    input,
    output,
    destination,
    metadata,
    binary,
    freeze: () => Effect.runPromise(freeze(input, destination, output))
  }
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
  compile(join(directory, "libfreeze_fixture.so"), "int fixture(void) { return 0; }", ["-shared", "-fPIC"])
const linked = (directory: string, lib: string, rpath: string) =>
  compile(join(directory, "program"), "extern int fixture(void); int main(void) { return fixture(); }", [
    `-L${dirname(lib)}`,
    "-lfreeze_fixture",
    `-Wl,-rpath,${rpath}`
  ])

test("generic metadata preserves unknown fields, collision-free independent bytes, hashes and modes", async () => {
  const f = await fixture(),
    sources = await Promise.all(Array.from({ length: 9 }, (_, index) => f.binary(`source-${index}/same-name`)))
  const original = await f.metadata(sources)
  await f.freeze()
  const frozen = await loadMetadata(f.output),
    manifest = object(uniqueJson(await fs.readFile(f.output + ".manifest.json", "utf8")))
  const copies = Object.values(frozen["rust-binaries"]).map((entry) => entry["binary-path"])
  expect(new Set(copies).size).toBe(sources.length)
  for (const [index, [identity, binary]] of Object.entries(frozen["rust-binaries"]).entries()) {
    const source = sources[index]!, copy = binary["binary-path"]
    expect((await fs.stat(copy)).ino).not.toBe((await fs.stat(source)).ino)
    expect((await fs.stat(copy)).mode & 0o777).toBe(0o751)
    expect(await digest(copy)).toBe(await digest(source))
    expect(object(object(manifest.binaries)[identity]).sha256).toBe(await digest(source))
    binary["binary-path"] = source
  }
  expect(frozen).toEqual(original)
  await fs.writeFile(sources[0]!, "compiler overwrite")
  await fs.unlink(sources[1]!)
  await Effect.runPromise(verify(f.output))
})

test("later missing binary never publishes partial metadata or manifest", async () => {
  const f = await fixture()
  await f.metadata([await f.binary(), join(f.target, "missing")])
  await expect(f.freeze()).rejects.toThrow()
  expect(await fs.exists(f.output)).toBe(false)
  expect(await fs.exists(f.output + ".manifest.json")).toBe(false)
  expect((await fs.readdir(join(f.destination, "0"))).length).toBe(1)
})

test("CLI publishes the manifest before the atomic metadata commit marker", async () => {
  const f = await fixture()
  await f.metadata([await f.binary()])
  const events: string[] = []
  const watcher = watch(f.directory, (_event, filename) => {
    if (filename) events.push(filename.toString())
  })
  const helper = join(import.meta.dirname, "freeze-binaries.ts")
  try {
    const child = Bun.spawn([
      process.execPath,
      helper,
      "--metadata",
      f.input,
      "--destination",
      f.destination,
      "--output",
      f.output
    ], { stdout: "pipe", stderr: "pipe" })
    expect(await child.exited, await new Response(child.stderr).text()).toBe(0)
    await Bun.sleep(50)
    expect(events.indexOf("binaries.json.manifest.json")).toBeGreaterThanOrEqual(0)
    expect(events.indexOf("binaries.json")).toBeGreaterThan(events.indexOf("binaries.json.manifest.json"))
    const verified = Bun.spawn([process.execPath, helper, "--verify", f.output], {
      env: { ...process.env, PATH: "/no-tools" },
      stdout: "pipe",
      stderr: "pipe"
    })
    expect(await verified.exited).toBe(0)
    expect(await new Response(verified.stdout).text()).not.toContain("\"ldd\"")
    const invalid = Bun.spawn([process.execPath, helper, "--verify", f.input], { stdout: "pipe", stderr: "pipe" })
    expect(await invalid.exited).toBe(1)
    expect(await new Response(invalid.stderr).text()).toContain("freeze-binaries:")
  } finally {
    watcher.close()
  }
})

test("malformed, duplicate, empty and nonabsolute metadata fail closed", async () => {
  const f = await fixture()
  for (const text of ["", "{", "null", "[]", "{}", "{\"rust-binaries\":{},\"rust-binaries\":{}}"]) {
    await fs.writeFile(f.input, text)
    await expect(f.freeze()).rejects.toThrow()
    expect(await fs.exists(f.output)).toBe(false)
  }
  await f.metadata([])
  await expect(f.freeze()).rejects.toThrow("nonempty rust-binaries")
  const source = await f.binary()
  for (
    const [field, value] of [["binary-path", "relative"], ["binary-id", "different"], ["package-id", null], [
      "kind",
      []
    ]] as const
  ) {
    const metadata = await f.metadata([source])
    Object.assign(Object.values(metadata["rust-binaries"])[0]!, { [field]: value })
    await fs.writeFile(f.input, JSON.stringify(metadata))
    await expect(f.freeze()).rejects.toThrow()
  }
})

test("rejects non-ELF, truncated ELF, empty and non-executable inputs", async () => {
  const f = await fixture(), source = await f.binary(), truncated = (await fs.readFile(source)).subarray(0, 64)
  for (
    const [bytes, mode] of [[Buffer.alloc(0), 0o755], [Buffer.from("#!/bin/sh\nexit 0\n"), 0o755], [truncated, 0o755], [
      Buffer.from("text"),
      0o644
    ]] as const
  ) {
    await fs.rm(f.destination, { recursive: true, force: true })
    await fs.writeFile(source, bytes)
    await fs.chmod(source, mode)
    await f.metadata([source])
    await expect(f.freeze()).rejects.toThrow()
    expect(await fs.exists(f.output)).toBe(false)
  }
})

test("fresh destinations and output markers are mandatory", async () => {
  const f = await fixture()
  await f.metadata([await f.binary()])
  await fs.mkdir(f.destination)
  await expect(f.freeze()).rejects.toThrow("destination already exists")
  await fs.rmdir(f.destination)
  await fs.writeFile(f.output, "existing success")
  await expect(f.freeze()).rejects.toThrow("output already exists")
  expect(await fs.readFile(f.output, "utf8")).toBe("existing success")
})

test("real static ELF is accepted", async () => {
  const f = await fixture()
  await f.metadata([await compile(join(f.target, "static"), "int main(void) { return 0; }", ["-static"])])
  await f.freeze()
  await Effect.runPromise(verify(f.output))
})

test("loader rejects unresolved, Cargo-managed and symlinked Cargo dependencies", async () => {
  for (const kind of ["unresolved", "managed", "symlink"]) {
    const f = await fixture(), lib = await library(join(kind === "unresolved" ? f.directory : f.target, "lib"))
    let search = dirname(lib)
    if (kind === "symlink") {
      search = join(f.directory, "external")
      await fs.mkdir(search)
      await fs.symlink(lib, join(search, "libfreeze_fixture.so"))
    }
    const source = await linked(join(f.directory, "runtime"), lib, search)
    if (kind === "unresolved") await fs.unlink(lib)
    await f.metadata([source])
    await expect(f.freeze()).rejects.toThrow(
      kind === "unresolved" ? "unresolved dependencies" : "dependency under Cargo-managed target"
    )
    expect(await fs.exists(f.output)).toBe(false)
  }
})

test("external dependency survives relocation but changed ORIGIN resolution is rejected", async () => {
  for (const kind of ["external", "origin-missing", "origin-fallback"]) {
    const f = await fixture(), lib = await library(join(f.directory, "runtime/source"))
    const fallback = kind === "origin-fallback" ? await library(join(f.directory, "fallback")) : undefined
    const source = await linked(
      dirname(lib),
      lib,
      kind === "external" ? dirname(lib) : `$ORIGIN${fallback ? ":" + dirname(fallback) : ""}`
    )
    await f.metadata([source])
    if (kind === "external") {
      await f.freeze()
      await Effect.runPromise(verify(f.output))
    } else {
      await expect(f.freeze()).rejects.toThrow(
        kind === "origin-missing" ? "unresolved dependencies" : "dependency resolution changed"
      )
      expect(await fs.exists(f.output)).toBe(false)
    }
  }
})

test("verify is read-only and tool-free and detects all commit-contract tampering", async () => {
  const f = await fixture()
  await f.metadata([await f.binary()])
  await f.freeze()
  const metadata = await fs.readFile(f.output),
    manifest = await fs.readFile(f.output + ".manifest.json"),
    frozen = await loadMetadata(f.output),
    copy = Object.values(frozen["rust-binaries"])[0]!["binary-path"],
    bytes = await fs.readFile(copy)
  const saved = process.env.PATH
  process.env.PATH = join(f.directory, "no-tools")
  try {
    await Effect.runPromise(verify(f.output))
    for (const change of ["bytes", "mode", "metadata", "manifest", "missing", "symlink"]) {
      if (change === "bytes") await fs.writeFile(copy, "tampered")
      if (change === "mode") await fs.chmod(copy, 0o700)
      if (change === "metadata") await fs.appendFile(f.output, " ")
      if (change === "manifest") {
        await fs.writeFile(
          f.output + ".manifest.json",
          JSON.stringify({ ...JSON.parse(manifest.toString()), binaries: {} })
        )
      }
      if (change === "missing" || change === "symlink") {
        await fs.unlink(copy)
        if (change === "symlink") await fs.symlink(join(f.target, "source/program"), copy)
      }
      await expect(Effect.runPromise(verify(f.output))).rejects.toThrow()
      await fs.rm(copy, { force: true })
      await fs.writeFile(copy, bytes)
      await fs.chmod(copy, 0o751)
      await fs.writeFile(f.output, metadata)
      await fs.writeFile(f.output + ".manifest.json", manifest)
    }
  } finally {
    process.env.PATH = saved
  }
})

test("snapshot manifest boundaries validate every required field before filesystem verification", async () => {
  const f = await fixture()
  await f.metadata([await f.binary()])
  await f.freeze()
  const path = f.output + ".manifest.json", manifest = object(uniqueJson(await fs.readFile(path, "utf8")))
  for (
    const fields of [{ version: "1" }, { "snapshot-directory": null }, { "metadata-sha256": null }, {
      binaries: { "package-0::generic-test": { "binary-path": 1, sha256: "secret-marker", mode: "755" } }
    }]
  ) {
    await fs.writeFile(path, JSON.stringify({ ...manifest, ...fields }))
    await expect(Effect.runPromise(verify(f.output))).rejects.toThrow("invalid snapshot manifest")
  }
  await fs.writeFile(path, "{\"secret-marker\":1,\"secret-marker\":2}")
  await expect(Effect.runPromise(verify(f.output))).rejects.toThrow("duplicate metadata key")
})
