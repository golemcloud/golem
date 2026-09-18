import { execFileSync, spawnSync } from "node:child_process"
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, relative, resolve, sep } from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"

const packageDirectory = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const npmCli = process.env.npm_execpath

if (!npmCli) throw new Error("npm_execpath is required; run this script through npm")

const temporaryDirectory = mkdtempSync(join(tmpdir(), "effect-golem-package-"))

const run = (command, args, options = {}) =>
  execFileSync(command, args, { encoding: "utf8", stdio: "pipe", ...options })

const runNpm = (args, options = {}) => run(process.execPath, [npmCli, ...args], options)

const walk = (directory) =>
  readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name)
    return entry.isDirectory() ? walk(path) : [path]
  })

const matchesSubpathPattern = (subpath, pattern) => {
  const [prefix, suffix = ""] = pattern.split("*")
  return pattern.includes("*")
    ? subpath.startsWith(prefix) && subpath.endsWith(suffix)
    : subpath === pattern
}

const declarationExports = (source) => {
  const modules = new Map()
  for (const declaration of source.matchAll(/declare\s+module\s+['"]([^'"]+)['"]\s*\{/g)) {
    const names = modules.get(declaration[1]) ?? new Set()
    let depth = 1
    let end = declaration.index + declaration[0].length
    for (; end < source.length && depth > 0; end += 1) {
      if (source[end] === "{") depth += 1
      if (source[end] === "}") depth -= 1
    }
    const body = source.slice(declaration.index + declaration[0].length, end - 1)
    let bodyDepth = 0
    for (const line of body.split("\n")) {
      if (bodyDepth === 0) {
        const exported = line.match(
          /^\s*export\s+(?:declare\s+)?(?:class|function|const|let|var|namespace|enum)\s+([A-Za-z_$][\w$]*)/,
        )
        if (exported) names.add(exported[1])
        const list = line.match(/^\s*export\s*\{([^}]+)\}/)
        if (list) {
          for (const item of list[1].split(",")) {
            const name = item
              .trim()
              .split(/\s+as\s+/)
              .at(-1)
            if (name && !name.startsWith("type ")) names.add(name)
          }
        }
      }
      bodyDepth += (line.match(/\{/g) ?? []).length - (line.match(/\}/g) ?? []).length
    }
    modules.set(declaration[1], names)
  }
  return modules
}

try {
  const dryRun = JSON.parse(runNpm(["pack", "--dry-run", "--json"], { cwd: packageDirectory }))
  const files = dryRun[0].files.map(({ path }) => path.replaceAll("\\", "/"))
  const forbidden = files.filter(
    (path) => path.endsWith(".map") || path === "wasm/artifact-provenance.json",
  )
  if (forbidden.length > 0) throw new Error(`Private package files:\n${forbidden.join("\n")}`)

  const packed = JSON.parse(
    runNpm(["pack", "--json", "--pack-destination", temporaryDirectory], {
      cwd: packageDirectory,
    }),
  )
  const tarball = join(temporaryDirectory, packed[0].filename)
  runNpm(["init", "--yes"], { cwd: temporaryDirectory })
  runNpm(["install", "--ignore-scripts", tarball], { cwd: temporaryDirectory })

  const installed = join(temporaryDirectory, "node_modules", "@golemcloud", "effect-golem")
  const manifest = JSON.parse(readFileSync(join(installed, "package.json"), "utf8"))
  runNpm(
    [
      "install",
      "--ignore-scripts",
      "--no-save",
      `@types/node@${manifest.devDependencies["@types/node"]}`,
      `typescript@${manifest.devDependencies.typescript}`,
    ],
    {
      cwd: temporaryDirectory,
    },
  )
  const explicitPublicModules = [
    ...Object.entries(manifest.exports)
      .filter(
        ([, target]) =>
          target !== null &&
          typeof target !== "string" &&
          target.import &&
          !target.import.includes("*"),
      )
      .map(([name]) => (name === "." ? manifest.name : `${manifest.name}/${name.slice(2)}`)),
  ]
  const nullExports = Object.entries(manifest.exports)
    .filter(([, target]) => target === null)
    .map(([subpath]) => subpath)
  const wildcardPublicModules = walk(join(installed, "dist", "src"))
    .filter((path) => path.endsWith(".js"))
    .map(
      (path) =>
        `./${relative(join(installed, "dist", "src"), path)
          .split(sep)
          .join("/")
          .slice(0, -3)}`,
    )
    .filter((subpath) => !nullExports.some((pattern) => matchesSubpathPattern(subpath, pattern)))
    .map((subpath) => `${manifest.name}/${subpath.slice(2)}`)
  const publicModules = [...explicitPublicModules, ...wildcardPublicModules]
  const uniquePublicModules = [...new Set(publicModules)].sort()

  for (const world of [
    "agent_guest.wasm",
    "tool_middleware_guest.wasm",
    "agent_tool_middleware_guest.wasm",
  ]) {
    const artifact = join(installed, "wasm", world)
    if (!statSync(artifact).isFile() || statSync(artifact).size < 8)
      throw new Error(`Invalid ${world}`)
    const magic = readFileSync(artifact).subarray(0, 4).toString("hex")
    if (magic !== "0061736d") throw new Error(`${world} is not a WebAssembly binary`)
  }

  const privateModules = [
    `${manifest.name}/internal/pipeable`,
    `${manifest.name}/host/HostLive`,
    `${manifest.name}/Mysql/internal/codec`,
  ]
  for (const moduleName of privateModules) {
    const result = spawnSync(
      process.execPath,
      ["--input-type=module", "--eval", `import(${JSON.stringify(moduleName)})`],
      {
        cwd: temporaryDirectory,
        encoding: "utf8",
      },
    )
    if (result.status === 0) throw new Error(`Private export is importable: ${moduleName}`)
    if (!result.stderr.includes("ERR_PACKAGE_PATH_NOT_EXPORTED")) {
      throw new Error(`Private export failed for the wrong reason: ${moduleName}\n${result.stderr}`)
    }
  }

  const hostExports = new Map()
  for (const path of walk(join(installed, "golem-types")).filter((path) =>
    path.endsWith(".d.ts"),
  )) {
    for (const [moduleName, names] of declarationExports(readFileSync(path, "utf8"))) {
      const exports = hostExports.get(moduleName) ?? new Set()
      for (const name of names) exports.add(name)
      hostExports.set(moduleName, exports)
    }
  }
  const nodeSqliteExports = hostExports.get("node:sqlite") ?? new Set()
  nodeSqliteExports.add("DatabaseSync")
  hostExports.set("node:sqlite", nodeSqliteExports)
  const loader = join(temporaryDirectory, "host-loader.mjs")
  writeFileSync(
    loader,
    `const modules = new Map(${JSON.stringify([...hostExports].map(([key, value]) => [key, [...value]]))});
export async function resolve(specifier, context, nextResolve) {
  if (specifier.startsWith("golem:") || specifier.startsWith("wasi:") || specifier === "node:sqlite") return { url: "golem-host:" + encodeURIComponent(specifier), shortCircuit: true };
  return nextResolve(specifier, context);
}
export async function load(url, context, nextLoad) {
  if (!url.startsWith("golem-host:")) return nextLoad(url, context);
  const name = decodeURIComponent(url.slice(11));
  const exports = modules.get(name) ?? [];
  return { format: "module", shortCircuit: true, source: "const stub = new Proxy(function () {}, { get: (_, key) => key === Symbol.iterator ? function* () { while (true) yield stub } : key === 'then' ? undefined : stub, apply: () => stub, construct: () => stub });\\n" + exports.map((item) => "export const " + item + " = stub;").join("\\n") };
}`,
  )
  const runtime = spawnSync(
    process.execPath,
    [
      "--experimental-loader",
      pathToFileURL(loader).href,
      "--input-type=module",
      "--eval",
      uniquePublicModules.map((name) => `await import(${JSON.stringify(name)})`).join("\n"),
    ],
    { cwd: temporaryDirectory, encoding: "utf8" },
  )
  if (runtime.status !== 0) throw new Error(`Public runtime import failed:\n${runtime.stderr}`)

  const ambientModules = Object.keys(manifest.typesVersions["*"])
  const ambientEntries = ambientModules.filter((name) => name.endsWith("guest"))
  const source = [
    ...uniquePublicModules.map((name) => `import type {} from ${JSON.stringify(name)}`),
    ...ambientEntries.map((name) => `import ${JSON.stringify(`${manifest.name}/${name}`)}`),
    ...ambientModules
      .filter((name) => !name.endsWith("guest"))
      .map((name) => `import type {} from ${JSON.stringify(name)}`),
  ].join("\n")
  writeFileSync(join(temporaryDirectory, "package-smoke.ts"), source)
  writeFileSync(
    join(temporaryDirectory, "tsconfig.json"),
    JSON.stringify({
      compilerOptions: {
        strict: true,
        skipLibCheck: false,
        module: "NodeNext",
        moduleResolution: "NodeNext",
        noEmit: true,
      },
      files: ["package-smoke.ts"],
    }),
  )
  run(
    process.execPath,
    [
      join(temporaryDirectory, "node_modules", "typescript", "bin", "tsc"),
      "-p",
      join(temporaryDirectory, "tsconfig.json"),
    ],
    { cwd: temporaryDirectory },
  )
  console.log(
    `Package smoke-tested ${uniquePublicModules.length} public exports from ${relative(packageDirectory, tarball).split(sep).join("/")}`,
  )
} finally {
  rmSync(temporaryDirectory, { recursive: true, force: true })
}
