/** Source-only exported-WASM cache. Publication follows successful fixture checks. */
import { Data, Effect, Schema } from "effect"
import { createHash } from "node:crypto"
import * as fs from "node:fs/promises"
import { basename, dirname, join, relative, resolve, sep } from "node:path"
import { fileURLToPath } from "node:url"
import { parse } from "smol-toml"

const SCHEMA = 2
const keyPattern = /^[0-9a-f]{64}$/
const Sources = Schema.Record(
  Schema.String,
  Schema.Union([Schema.String, Schema.Tuple([Schema.String, Schema.Number])])
)
type Sources = typeof Sources.Type
const Stamps = Schema.Record(Schema.String, Schema.String)
type Stamps = typeof Stamps.Type
const Inventory = Schema.Record(
  Schema.String,
  Schema.Struct({ sha256: Schema.String, size: Schema.Number, mode: Schema.Number })
)
type Inventory = typeof Inventory.Type
const Completion = Schema.Struct({ schema: Schema.Number, key: Schema.String, files: Inventory })
const ReceiptKey = Schema.Struct({ key: Schema.String })
const Receipt = Schema.Struct({ key: Schema.String, before: Stamps, sources: Sources })
export class CacheError extends Data.TaggedError("CacheError")<{ message: string }> {}
function decode<S extends Schema.Top & { readonly DecodingServices: never }>(schema: S, value: unknown): S["Type"] {
  try {
    return Schema.decodeUnknownSync(schema)(value)
  } catch {
    return fail("invalid cache metadata")
  }
}
const io = <A>(run: () => Promise<A>) =>
  Effect.tryPromise({
    try: run,
    catch: (error) => {
      if (error instanceof CacheError) return error
      // TOML/JSON parser errors can quote input contents. OS errors can quote paths.
      const code = error !== null && typeof error === "object" && "code" in error && typeof error.code === "string" &&
          /^E[A-Z0-9]+$/.test(error.code)
        ? error.code
        : "invalid data"
      return new CacheError({ message: `I/O or parsing failure (${code})` })
    }
  })
const fail = (message: string): never => {
  throw new CacheError({ message })
}
function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`
  if (value !== null && typeof value === "object") {
    return `{${
      Object.entries(value).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0).map(([k, v]) =>
        `${JSON.stringify(k)}:${canonical(v)}`
      ).join(",")
    }}`
  }
  return JSON.stringify(value) ?? "null"
}
export const identity = (value: unknown) => createHash("sha256").update(canonical(value)).digest("hex")
const digest = async (path: string) => createHash("sha256").update(await fs.readFile(path)).digest("hex")
const inside = (root: string, path: string) => {
  const r = relative(root, path)
  return r === "" || (!r.startsWith(`..${sep}`) && r !== ".." && !r.startsWith(sep))
}
const object = (value: unknown): Record<string, unknown> => decode(Schema.Record(Schema.String, Schema.Unknown), value)
const json = async (path: string): Promise<unknown> => JSON.parse(await fs.readFile(path, "utf8"))
const regularDirectory = async (path: string) => {
  if (!(await fs.lstat(path)).isDirectory()) fail("expected regular directory")
}
async function resolveSource(path: string, links = new Set<string>()): Promise<string> {
  const stat = await fs.lstat(path).catch((error: unknown) => {
    if (error !== null && typeof error === "object" && "code" in error && error.code === "ENOENT") return undefined
    throw error
  })
  if (stat?.isSymbolicLink()) {
    if (links.has(path) || links.size >= 40) fail("source symlink cycle")
    links.add(path)
    return resolveSource(resolve(dirname(path), await fs.readlink(path)), links)
  }
  if (stat) return fs.realpath(path)
  const parent = dirname(path)
  return parent === path ? path : join(await resolveSource(parent, links), basename(path))
}
export const command = Effect.fn("fixtureCache.command")(function*(
  args: string[],
  cwd: string,
  env: NodeJS.ProcessEnv = process.env,
  timeout = 30_000
) {
  const child = yield* Effect.acquireRelease(
    io(async () => Bun.spawn(args, { cwd, env, timeout, stdout: "pipe", stderr: "pipe" })),
    (child) =>
      io(async () => {
        if (child.exitCode === null) child.kill("SIGKILL")
        await child.exited
      }).pipe(Effect.orDie)
  )
  return yield* io(async () => {
    const [stdout, , code] = await Promise.all([
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
      child.exited
    ])
    if (code !== 0) fail(`command ${basename(args[0] ?? "")} failed (${code})`)
    return stdout
  })
}, Effect.scoped)

export const sourceInputs = Effect.fn("fixtureCache.sourceInputs")(function*(root: string, app: string) {
  if (basename(app) !== app || ["", ".", ".."].includes(app) || app.includes("\\")) {
    return yield* Effect.fail(new CacheError({ message: "invalid application directory" }))
  }
  const tracked = (yield* command(["git", "ls-files", "-z"], root)).split("\0").filter(Boolean)
  const component = `test-components/${app}`
  if (!tracked.some((p) => p.startsWith(`${component}/`))) {
    return yield* Effect.fail(new CacheError({ message: "application has no checked-in source" }))
  }
  return yield* io(async () => {
    const workspace = object(object(parse(await fs.readFile(join(root, "Cargo.toml"), "utf8"))).workspace)
    const dependencies = object(workspace.dependencies ?? {})
    const pending = [join(root, "cli/golem-cli/Cargo.toml")]
    const manifests = new Set<string>()
    while (pending.length) {
      const manifest = await fs.realpath(pending.pop()!)
      if (!inside(root, manifest)) fail("CLI dependency outside checkout")
      if (manifests.has(manifest)) continue
      manifests.add(manifest)
      const pkg = object(parse(await fs.readFile(manifest, "utf8")))
      for (const table of [pkg, ...Object.values(object(pkg.target ?? {})).map(object)]) {
        for (const kind of ["dependencies", "build-dependencies"]) {
          for (const [name, original] of Object.entries(object(table[kind] ?? {}))) {
            if (typeof original !== "object" || original === null) continue
            let spec = object(original), base = dirname(manifest)
            if (spec.workspace) {
              const inherited = dependencies[name]
              if (typeof inherited === "string") continue
              spec = object(inherited)
              base = root
            }
            if (typeof spec.path === "string") pending.push(resolve(base, spec.path, "Cargo.toml"))
          }
        }
      }
    }
    // CLI migrations and generators can read embedded skills and templates. Keep
    // these broad inputs until their exported-WASM dependency contract is explicit.
    const roots = [
      "sdks",
      "wit",
      "cli",
      "golem-skills/skills",
      "openapi",
      ".cargo",
      ".github/actions",
      component,
      ...[...manifests].map((p) => relative(root, dirname(p)))
    ]
    const paths = tracked.filter((p) =>
      ["Cargo.toml", "Cargo.lock"].includes(p) || p.startsWith("Makefile") || roots.some((r) =>
        p.startsWith(`${r}/`)
      ) || (p.startsWith("test-components/") && p.split("/").length === 2)
    )
    const files: Record<string, Sources[string]> = {}
    while (paths.length) {
      const name = paths.pop()!
      if (name in files) continue
      const path = join(root, name), stat = await fs.lstat(path)
      if (stat.isSymbolicLink()) {
        // Generated SDK directory links can be dangling before the first build.
        const link = await fs.readlink(path)
        const lexical = resolve(dirname(path), link)
        if (!inside(root, lexical)) fail("source symlink outside checkout")
        const target = await resolveSource(path)
        if (!inside(root, target)) fail("source symlink outside checkout")
        files[name] = link
        const r = relative(root, target)
        paths.push(...tracked.filter((p) => p === r || p.startsWith(`${r}/`)))
      } else {
        if (!stat.isFile()) fail("source is not a regular file")
        if (!inside(root, await fs.realpath(path))) fail("source outside checkout")
        files[name] = [await digest(path), stat.mode & 0o777]
      }
    }
    return files
  })
})
export const fingerprint = Effect.fn("fixtureCache.fingerprint")(
  function*(root: string, app: string, variant: string, sources?: Sources) {
    const files = sources ?? (yield* sourceInputs(root, app))
    const helper = yield* io(() => digest(fileURLToPath(import.meta.url)))
    return identity({ schema: SCHEMA, app, variant, sources: files, helper })
  }
)
export const stamps = (root: string) =>
  io(async () => {
    const result: Record<string, Stamps[string]> = {}
    for (const name of await fs.readdir(join(root, "test-components"))) {
      if (name.endsWith(".wasm")) {
        const stat = await fs.lstat(join(root, "test-components", name), { bigint: true })
        result[name] = `${stat.size}:${stat.mtimeNs}:${stat.ctimeNs}`
      }
    }
    return result
  })
const inventory = async (path: string): Promise<Inventory> => {
  await regularDirectory(path)
  const result: Record<string, Inventory[string]> = {}
  for (const name of await fs.readdir(path)) {
    const file = join(path, name), stat = await fs.lstat(file)
    if (!stat.isFile()) fail("bundle contains non-regular files")
    result[name] = { sha256: await digest(file), size: stat.size, mode: stat.mode & 0o777 }
  }
  return result
}
export const verify = (bundle: string, key: string) =>
  io(async () => {
    if (!keyPattern.test(key)) fail("invalid cache key")
    await regularDirectory(bundle)
    if (!(await fs.lstat(join(bundle, "COMPLETE.json"))).isFile()) fail("invalid completion marker")
    const raw = await json(join(bundle, "COMPLETE.json"))
    const manifest = decode(Completion, raw)
    // Reject fields the decoder would otherwise strip from the exact inventory contract.
    if (canonical(object(raw).files) !== canonical(manifest.files)) fail("invalid output inventory")
    if (manifest.schema !== SCHEMA || manifest.key !== key) fail("bundle identity mismatch")
    const files = manifest.files
    if (
      !Object.keys(files).length ||
      Object.keys(files).some((name) => basename(name) !== name || name.includes("\\") || !name.endsWith(".wasm"))
    ) fail("invalid output inventory")
    const actual = await inventory(join(bundle, "files"))
    if (canonical(actual) !== canonical(files) || Object.values(actual).some((info) => !info.size)) {
      fail("bundle incomplete or corrupt")
    }
    return actual
  })
const copy = async (source: string, destination: string) => {
  await fs.copyFile(source, destination)
  await fs.chmod(destination, (await fs.stat(source)).mode & 0o777)
}
export const restore = Effect.fn("fixtureCache.restore")(function*(root: string, bundle: string, key: string) {
  const files = yield* verify(bundle, key)
  yield* io(async () => {
    await regularDirectory(join(root, "test-components"))
    for (const name of Object.keys(files)) {
      const target = join(root, "test-components", name)
      await fs.rm(target, { force: true })
      await copy(join(bundle, "files", name), target)
    }
  })
})
export const stage = (root: string, pending: string, key: string, before: Stamps) =>
  Effect.scoped(Effect.gen(function*() {
    if (!keyPattern.test(key)) return yield* Effect.fail(new CacheError({ message: "invalid cache key" }))
    const after = yield* stamps(root)
    const outputs = Object.keys(after).filter((name) => after[name] !== before[name])
    if (!outputs.length) return yield* Effect.fail(new CacheError({ message: "build exported no WASMs" }))
    const temporary = yield* Effect.acquireRelease(
      io(() => fs.mkdtemp(join(pending, ".stage-"))),
      (path) => io(() => fs.rm(path, { recursive: true, force: true })).pipe(Effect.orDie)
    )
    yield* Effect.gen(function*() {
      const bundle = join(temporary, "bundle"), payload = join(bundle, "files")
      yield* io(async () => {
        await fs.mkdir(payload, { recursive: true })
        for (const name of outputs) {
          const source = join(root, "test-components", name)
          if (!(await fs.lstat(source)).isFile()) fail("exported WASM is not a regular file")
          await copy(source, join(payload, name))
        }
        await fs.writeFile(
          join(bundle, "COMPLETE.json"),
          canonical({ schema: SCHEMA, key, files: await inventory(payload) })
        )
      })
      yield* verify(bundle, key)
      yield* io(async () => {
        await fs.rm(join(pending, key), { recursive: true, force: true })
        await fs.rename(bundle, join(pending, key))
      })
    })
  }))
export const publish = Effect.fn("fixtureCache.publish")(function*(pending: string, cache: string) {
  yield* io(() => fs.mkdir(cache, { recursive: true }))
  const retained = new Set<string>()
  const names = yield* io(() => fs.readdir(pending))
  for (const name of names.filter((name) => name.endsWith(".json"))) {
    const receipt = yield* io(async () => decode(ReceiptKey, await json(join(pending, name))))
    if (!keyPattern.test(receipt.key)) {
      return yield* Effect.fail(new CacheError({ message: "invalid cache receipt key" }))
    }
    retained.add(receipt.key)
  }
  for (const key of names.sort().filter((name) => !name.startsWith(".") && !name.endsWith(".json"))) {
    yield* verify(join(pending, key), key)
    retained.add(key)
    yield* Effect.scoped(Effect.gen(function*() {
      const temporary = yield* Effect.acquireRelease(
        io(() => fs.mkdtemp(join(cache, ".publish-"))),
        (path) => io(() => fs.rm(path, { recursive: true, force: true })).pipe(Effect.orDie)
      )
      const copied = join(temporary, "bundle")
      yield* io(() => fs.cp(join(pending, key), copied, { recursive: true }))
      yield* verify(copied, key)
      yield* io(async () => {
        await fs.rm(join(cache, key), { recursive: true, force: true })
        await fs.rename(copied, join(cache, key))
      })
    }))
    console.log(`Fixture cache published: ${key}`)
  }
  for (const key of yield* io(() => fs.readdir(cache))) {
    if (keyPattern.test(key) && !retained.has(key)) {
      yield* io(() => fs.rm(join(cache, key), { recursive: true, force: true }))
      console.log(`Fixture cache evicted: ${key}`)
    }
  }
})

export const run = Effect.fn("fixtureCache.run")(
  function*(root: string, cache: string, operation: string, app = "", variant = "release") {
    const pending = join(dirname(cache), "pending")
    if (operation === "begin") {
      yield* io(async () => {
        await fs.rm(pending, { recursive: true, force: true })
        await fs.mkdir(pending, { recursive: true })
      })
      return 0
    }
    if (operation === "publish") {
      yield* publish(pending, cache)
      return 0
    }
    if (!["restore", "stage"].includes(operation)) {
      return yield* Effect.fail(new CacheError({ message: "invalid operation" }))
    }
    return yield* Effect.gen(function*() {
      const sources = yield* sourceInputs(root, app)
      const key = yield* fingerprint(root, app, variant, sources)
      const receipt = join(pending, `${identity([app, variant])}.json`)
      console.log(
        `Fixture cache inputs: ${app} (${variant}) key=${key} files=${Object.keys(sources).length} source-digest=${
          identity(sources)
        }`
      )
      const groups = new Map<string, Record<string, Sources[string]>>()
      for (const [path, value] of Object.entries(sources)) {
        const group = path.startsWith("test-components/") ? path.split("/").slice(0, 2).join("/") : path.split("/")[0]!
        const members = groups.get(group) ?? {}
        members[path] = value
        groups.set(group, members)
      }
      console.log(
        `Fixture cache input groups: ${
          canonical(
            Object.fromEntries(
              [...groups].map((
                [name, members]
              ) => [name, { files: Object.keys(members).length, digest: identity(members) }])
            )
          )
        }`
      )
      if (operation === "restore") {
        const before = yield* stamps(root)
        yield* io(() => fs.writeFile(receipt, canonical({ key, before, sources })))
        yield* restore(root, join(cache, key), key)
        console.log(`Fixture cache hit: ${app} (${variant}) ${key}`)
      } else {
        const previous = yield* io(async () => decode(Receipt, await json(receipt)))
        if (previous.key !== key) {
          const old = previous.sources
          const changed = [...new Set([...Object.keys(old), ...Object.keys(sources)])].filter((p) =>
            canonical(old[p]) !== canonical(sources[p])
          ).sort()
          return yield* Effect.fail(
            new CacheError({
              message: `checked-in build inputs changed during compilation: ${
                changed.map((p) => JSON.stringify(p)).join(", ")
              }`
            })
          )
        }
        yield* stage(root, pending, key, previous.before)
      }
      return 0
    }).pipe(Effect.catch((error) => {
      // Paths and key metadata only: never print source or subprocess contents.
      console.log(`Fixture cache ${operation} miss: ${app} (${variant}): ${JSON.stringify(error.message)}`)
      return Effect.succeed(operation === "restore" ? 1 : 0)
    }))
  }
)
if (import.meta.main) {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
  const cache = process.env.GOLEM_FIXTURE_CACHE
  if (!cache) {
    console.error("GOLEM_FIXTURE_CACHE is required")
    process.exitCode = 1
  } else {process.exitCode = await Effect.runPromise(
      run(root, resolve(cache), process.argv[2] ?? "", process.argv[3], process.argv[4])
    ).catch(() => {
      console.error("Fixture cache operation failed")
      return 1
    })}
}
