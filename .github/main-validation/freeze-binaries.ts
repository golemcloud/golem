#!/usr/bin/env bun
import { Effect, Schema } from "effect"
import { spawn } from "node:child_process"
import * as fs from "node:fs/promises"
import { basename, dirname, isAbsolute, join, relative, resolve } from "node:path"
import { cleanupGroup } from "./process.ts"
import { decode, digest, errno, hash, io, publish, uniqueJson, ValidationError } from "./tooling-io.ts"

const Binary = Schema.StructWithRest(
  Schema.Struct({
    "binary-id": Schema.String,
    "binary-name": Schema.String,
    "package-id": Schema.String,
    kind: Schema.String,
    "binary-path": Schema.mutableKey(Schema.String),
    "build-platform": Schema.String
  }),
  [Schema.Record(Schema.String, Schema.Unknown)]
)
const Metadata = Schema.StructWithRest(
  Schema.Struct({
    "rust-build-meta": Schema.StructWithRest(Schema.Struct({ "target-directory": Schema.String }), [
      Schema.Record(Schema.String, Schema.Unknown)
    ]),
    "rust-binaries": Schema.Record(Schema.String, Binary)
  }),
  [Schema.Record(Schema.String, Schema.Unknown)]
)
type Metadata = typeof Metadata.Type
const Entry = Schema.Struct({ "binary-path": Schema.String, sha256: Schema.String, mode: Schema.Number })
const Manifest = Schema.Struct({
  version: Schema.Literal(1),
  "snapshot-directory": Schema.String,
  binaries: Schema.Record(Schema.String, Entry),
  "metadata-sha256": Schema.String
})
type Manifest = typeof Manifest.Type
const invalid = (message: string) => new ValidationError({ message })
const inside = (path: string, root: string) => {
  const rel = relative(root, path)
  return rel === "" || (!rel.startsWith("../") && rel !== ".." && !isAbsolute(rel))
}
const exists = async (path: string) => {
  try {
    await fs.lstat(path)
    return true
  } catch (error) {
    if (errno(error) === "ENOENT") return false
    throw error
  }
}

export async function loadMetadata(path: string): Promise<Metadata> {
  const value = decode(Metadata, uniqueJson(await fs.readFile(path, "utf8")), "invalid test binary metadata")
  const build = value["rust-build-meta"]
  if (!isAbsolute(build["target-directory"])) throw invalid("target-directory must be absolute")
  const binaries = value["rust-binaries"]
  if (!Object.keys(binaries).length) throw invalid("metadata requires nonempty rust-binaries")
  for (const [identity, entry] of Object.entries(binaries)) {
    const binary = entry
    for (const field of ["binary-id", "binary-name", "package-id", "kind", "binary-path", "build-platform"] as const) {
      if (!binary[field]) throw invalid(`invalid ${field}`)
    }
    if (!identity || binary["binary-id"] !== identity || !isAbsolute(binary["binary-path"])) {
      throw invalid("invalid identity or binary-path")
    }
  }
  return value
}

export const dependencies = (path: string, managedTargets: string[]) =>
  Effect.scoped(Effect.gen(function*() {
    const segments = yield* io(async () => {
      const file = await fs.open(path, "r")
      try {
        const header = Buffer.alloc(64), { bytesRead } = await file.read(header, 0, 64, 0)
        const wide = header[4] === 2, little = header[5] === 1
        const u16 = (offset: number) => little ? header.readUInt16LE(offset) : header.readUInt16BE(offset)
        const u32 = (bytes: Buffer, offset: number) => little ? bytes.readUInt32LE(offset) : bytes.readUInt32BE(offset)
        if (
          bytesRead < (wide ? 64 : 52) || header.subarray(0, 4).toString() !== "\x7fELF" ||
          ![1, 2].includes(header[4]!) || ![1, 2].includes(header[5]!) || ![2, 3].includes(u16(16))
        ) throw invalid("not an ELF executable")
        const offset = wide ? Number(little ? header.readBigUInt64LE(32) : header.readBigUInt64BE(32)) : u32(header, 28)
        const size = u16(wide ? 54 : 42), count = u16(wide ? 56 : 44)
        if (
          size !== (wide ? 56 : 32) || !count || offset < (wide ? 64 : 52) || !Number.isSafeInteger(offset) ||
          offset + size * count > (await file.stat()).size
        ) throw invalid("invalid ELF program headers")
        const bytes = Buffer.alloc(size * count)
        if ((await file.read(bytes, 0, bytes.length, offset)).bytesRead !== bytes.length) throw invalid("truncated ELF")
        const segments = Array.from({ length: count }, (_, index) => u32(bytes, index * size))
        if (!segments.includes(1)) throw invalid("ELF has no loadable segments")
        return segments
      } finally {
        await file.close()
      }
    })
    const process = yield* Effect.acquireRelease(
      io(async () => {
        const environment: NodeJS.ProcessEnv = { ...globalThis.process.env, LC_ALL: "C" }
        delete environment.CI_DASHBOARD_PROGRESS_TOKEN
        const child = spawn("ldd", [path], { detached: true, env: environment, stdio: ["ignore", "pipe", "pipe"] })
        let evidence = ""
        child.stdout.on("data", (chunk: Buffer) => {
          evidence += chunk.toString()
        })
        child.stderr.on("data", (chunk: Buffer) => {
          evidence += chunk.toString()
        })
        const result = new Promise<{ code: number | null; evidence: string }>((resolve, reject) => {
          child.once("error", reject)
          child.once("close", (code) => resolve({ code, evidence }))
        })
        void result.catch(() => {})
        return { child, result }
      }),
      ({ child, result }) =>
        Effect.gen(function*() {
          if (child.pid) yield* cleanupGroup(child.pid).pipe(Effect.orDie)
          yield* io(() => result).pipe(Effect.timeoutOption("1 second"), Effect.ignore)
        })
    )
    const result = yield* io(() => process.result).pipe(Effect.timeout("60 seconds"))
    console.log(JSON.stringify({ binary: path, ldd_exit: result.code, ldd: result.evidence }))
    return yield* io(async () => {
      if (result.evidence.includes("not found")) throw invalid("unresolved dependencies")
      const lines = result.evidence.split(/\r?\n/).map((line) => line.trim()).filter(Boolean)
      if (lines.length === 1 && ["statically linked", "not a dynamic executable"].includes(lines[0]!)) {
        if (![0, 1].includes(result.code!) || segments.includes(3)) throw invalid("ldd failed")
        return {}
      }
      if (result.code !== 0 || !lines.length) throw invalid("ldd failed")
      const resolved: Record<string, [string, string]> = {}
      for (let line of lines) {
        line = line.replace(/\s+\(0x[0-9a-fA-F]+\)$/, "")
        if (/^linux-(vdso|gate)\.so\./.test(line)) continue
        const separator = line.indexOf(" => ")
        const name = separator < 0 ? line : line.slice(0, separator),
          library = separator < 0 ? line : line.slice(separator + 4)
        if (!isAbsolute(library) || !(await fs.stat(library)).isFile()) throw invalid("unsupported ldd resolution")
        const real = await fs.realpath(library)
        if (managedTargets.some((root) => inside(library, root) || inside(real, root))) {
          throw invalid("dependency under Cargo-managed target")
        }
        resolved[name] = [library, real]
      }
      if (!Object.keys(resolved).length) throw invalid("no dependency resolution from ldd")
      return resolved
    })
  }))

export async function checkSnapshots(metadata: Metadata, manifest: Manifest): Promise<void> {
  if (
    manifest.version !== 1 ||
    Object.keys(manifest.binaries).sort().join("\0") !== Object.keys(metadata["rust-binaries"]).sort().join("\0")
  ) throw invalid("snapshot manifest identities differ from metadata")
  const directory = manifest["snapshot-directory"]
  if (!isAbsolute(directory)) throw invalid("invalid snapshot directory")
  const realDirectory = await fs.realpath(directory)
  for (const [identity, binary] of Object.entries(metadata["rust-binaries"])) {
    const entry = manifest.binaries[identity], path = binary["binary-path"], stat = await fs.lstat(path)
    if (
      !entry || entry["binary-path"] !== path || !inside(await fs.realpath(path), realDirectory) ||
      stat.isSymbolicLink() || !stat.isFile()
    ) throw invalid("invalid snapshot path")
    const actual = await digest(path)
    console.log(JSON.stringify({ verify: identity, binary: path, sha256: actual }))
    if (actual !== entry.sha256 || (stat.mode & 0o7777) !== entry.mode) {
      throw invalid("snapshot content or permissions changed")
    }
  }
}

export const freeze = Effect.fn("freezeBinaries")(function*(metadataPath: string, destination: string, output: string) {
  const metadata = yield* io(() => loadMetadata(metadataPath))
  destination = resolve(destination)
  output = resolve(output)
  const manifestPath = output + ".manifest.json"
  const targets = yield* io(async () => {
    if (await exists(destination)) throw invalid("destination already exists")
    for (const path of [output, manifestPath]) if (await exists(path)) throw invalid("output already exists")
    const roots = [
      metadata["rust-build-meta"]["target-directory"],
      resolve("target"),
      resolve(import.meta.dirname, "../../target"),
      process.env.CARGO_TARGET_DIR
    ].filter((root): root is string => !!root)
    const targets = await Promise.all(roots.map(async (root) => fs.realpath(resolve(root)).catch(() => resolve(root))))
    await fs.mkdir(dirname(destination), { recursive: true })
    await fs.mkdir(destination)
    return targets
  })
  const binaries: Record<string, typeof Entry.Type> = {}
  let index = 0
  for (const [identity, binary] of Object.entries(metadata["rust-binaries"])) {
    const source = binary["binary-path"]
    const before = yield* io(async () => {
      const stat = await fs.stat(source)
      if (!stat.isFile() || !(stat.mode & 0o111) || !stat.size) throw invalid("not a nonempty executable file")
      return { stat, hash: await digest(source) }
    })
    const beforeDependencies = yield* dependencies(source, targets)
    const copied = join(destination, String(index++), basename(source))
    yield* io(async () => {
      await fs.mkdir(dirname(copied))
      await fs.copyFile(source, copied)
      await fs.chmod(copied, before.stat.mode & 0o7777)
      const stat = await fs.stat(copied)
      if (
        stat.ino === before.stat.ino && stat.dev === before.stat.dev ||
        (stat.mode & 0o7777) !== (before.stat.mode & 0o7777)
      ) throw invalid("copy did not preserve independent executable")
      if (await digest(copied) !== before.hash || await digest(source) !== before.hash) {
        throw invalid("binary changed during snapshot")
      }
    })
    const afterDependencies = yield* dependencies(copied, targets)
    if (JSON.stringify(afterDependencies) !== JSON.stringify(beforeDependencies)) {
      return yield* Effect.fail(invalid("dependency resolution changed after relocation"))
    }
    binary["binary-path"] = copied
    binaries[identity] = { "binary-path": copied, sha256: before.hash, mode: before.stat.mode & 0o7777 }
    console.log(
      JSON.stringify({
        "binary-id": identity,
        source,
        copy: copied,
        sha256: before.hash,
        dependencies: beforeDependencies
      })
    )
  }
  yield* io(async () => {
    const encoded = JSON.stringify(metadata) + "\n"
    const manifest: Manifest = {
      version: 1,
      "snapshot-directory": destination,
      binaries,
      "metadata-sha256": hash(encoded)
    }
    await checkSnapshots(metadata, manifest)
    // Metadata is the commit marker and is always published last.
    await publish(manifestPath, JSON.stringify(manifest) + "\n")
    await publish(output, encoded)
  })
})

export const verify = (path: string) =>
  io(async () => {
    const metadata = await loadMetadata(path),
      manifest = decode(
        Manifest,
        uniqueJson(await fs.readFile(path + ".manifest.json", "utf8")),
        "invalid snapshot manifest"
      )
    if (await digest(path) !== manifest["metadata-sha256"]) throw invalid("snapshot metadata hash changed")
    await checkSnapshots(metadata, manifest)
  })

if (import.meta.main) {
  const args = process.argv.slice(2), options = new Map<string, string>()
  for (let index = 0; index < args.length; index += 2) options.set(args[index]!, args[index + 1]!)
  try {
    if (options.size === 1 && options.get("--verify")) await Effect.runPromise(verify(options.get("--verify")!))
    else if (
      options.size === 3 && options.get("--metadata") && options.get("--destination") && options.get("--output")
    ) {
      await Effect.runPromise(
        freeze(options.get("--metadata")!, options.get("--destination")!, options.get("--output")!)
      )
    } else throw invalid("requires --verify METADATA or --metadata INPUT --destination DIRECTORY --output OUTPUT")
  } catch (error) {
    console.error(`freeze-binaries: ${String(error)}`)
    process.exitCode = 1
  }
}
