import { dirname, join, resolve, sep } from "node:path"
import { fileURLToPath } from "node:url"
import { sharedEffectRuntime } from "./shared-effect.mjs"
import { staticContracts } from "./static-contracts.mjs"

const sdkSource = resolve(dirname(fileURLToPath(import.meta.url)), "../dist/src")
const entry = "\0golem-effect-component"
const packageName = "@golemcloud/effect-golem"
const publicEntries = new Map([
  [packageName, "index.js"],
  [`${packageName}/HttpRouter`, "internal/component/HttpRouter.js"],
  [`${packageName}/middleware`, "Middleware.js"],
  [`${packageName}/sqlite`, "Sqlite/SqliteClient.js"],
  [`${packageName}/postgres`, "Postgres/PgClient.js"],
  [`${packageName}/mysql`, "Mysql/MySqlClient.js"],
  [`${packageName}/ignite2`, "Ignite/IgniteClient.js"],
])

const normalizePlugins = async (plugins) => {
  const normalized = []
  const visit = async (plugin) => {
    const resolved = await plugin
    if (!resolved) return
    if (Array.isArray(resolved)) {
      for (const nested of resolved) await visit(nested)
    } else normalized.push(resolved)
  }
  await visit(plugins)
  return normalized
}

/**
 * Build-time capability selection, based on the tree-shaken application graph.
 * Discovery does not execute application code. A retained definition module is
 * conservatively considered capable even if registration is conditional.
 */
export async function componentConfiguration(rollup, optionsFactory) {
  if (typeof optionsFactory !== "function")
    throw new Error("componentConfiguration expects an options factory")
  const probeOptions = optionsFactory()
  if (!probeOptions || typeof probeOptions.then === "function")
    throw new Error("The component options factory must return Rollup options synchronously")
  const options = probeOptions
  if (typeof options.input !== "string") throw new Error("Expected one component entrypoint")
  if (options.watch) throw new Error("Effect component builds do not support watch mode")
  if (options.cache) throw new Error("Effect component builds do not support Rollup cache")
  const input = resolve(options.input)
  const makeSdkPlugin = () => {
    let usesSourceHttpRouter = false
    return {
      name: "golem-effect-sdk-source",
      resolveId(source) {
        if (source === `${packageName}/HttpRouter`) usesSourceHttpRouter = true
        if (source === packageName)
          return {
            id: join(sdkSource, usesSourceHttpRouter ? "internal/component/index.js" : "index.js"),
            moduleSideEffects: false,
          }
        const path = publicEntries.get(source)
        if (path) return { id: join(sdkSource, path), moduleSideEffects: false }
        if (source.startsWith(sdkSource + sep)) return { id: source, moduleSideEffects: false }
        return null
      },
      transform(code, id) {
        if (id.startsWith(sdkSource + sep)) return { code, map: null, moduleSideEffects: false }
        return null
      },
    }
  }
  const plugins = [
    staticContracts(sdkSource, publicEntries),
    makeSdkPlugin(),
    sharedEffectRuntime(input),
    ...(await normalizePlugins(options.plugins)),
  ]
  const probe = await rollup({ ...options, input, plugins })
  let modules
  try {
    const { output } = await probe.generate({ format: "esm", inlineDynamicImports: true })
    modules = output
      .filter((item) => item.type === "chunk")
      .flatMap((item) =>
        Object.entries(item.modules)
          .filter(([, info]) => info.renderedLength > 0)
          .map(([id]) => id),
      )
  } finally {
    await probe.close()
  }
  const finalOptions = optionsFactory()
  if (!finalOptions || typeof finalOptions.then === "function")
    throw new Error("The component options factory must return Rollup options synchronously")
  if (typeof finalOptions.input !== "string" || resolve(finalOptions.input) !== input)
    throw new Error("The component options factory must return the same component entrypoint")
  if (finalOptions.watch) throw new Error("Effect component builds do not support watch mode")
  if (finalOptions.cache) throw new Error("Effect component builds do not support Rollup cache")
  const probeCallerPlugins = new Set(await normalizePlugins(options.plugins))
  const finalCallerPlugins = await normalizePlugins(finalOptions.plugins)
  if (finalCallerPlugins.some((plugin) => probeCallerPlugins.has(plugin)))
    throw new Error("The component options factory must create fresh plugin instances")
  const includes = (path) => modules.includes(join(sdkSource, path))
  const capabilities = {
    agents: includes("internal/agent.js"),
    tools: includes("internal/tool/registry.js"),
    middleware: includes("internal/tool/middleware.js"),
  }
  const from = (path) => JSON.stringify(join(sdkSource, path))
  const empty = from("internal/emptyGuest.js")
  const source = [
    `import ${JSON.stringify(input)};`,
    capabilities.agents
      ? `export { guest as golemAgent200Guest, saveSnapshot, loadSnapshot } from ${from("internal/guest.js")};`
      : `export { golemAgent200Guest, saveSnapshot, loadSnapshot } from ${empty};`,
    capabilities.tools
      ? `export { toolGuest as golemTool010Guest } from ${from("internal/tool/runtime.js")};`
      : `export { golemTool010Guest } from ${empty};`,
    capabilities.middleware
      ? `export { toolMiddlewareGuest } from ${from("internal/tool/middleware.js")};`
      : `export { toolMiddlewareGuest } from ${empty};`,
  ].join("\n")
  let started = false
  const lifecycle = {
    name: "golem-effect-one-shot",
    options(inputOptions) {
      if (started)
        throw new Error("Effect component configuration is one-shot; create a fresh configuration")
      if (inputOptions.watch) throw new Error("Effect component builds do not support watch mode")
      if (inputOptions.cache) throw new Error("Effect component builds do not support Rollup cache")
    },
    buildStart(inputOptions) {
      if (started)
        throw new Error("Effect component configuration is one-shot; create a fresh configuration")
      if (this.meta.watchMode) throw new Error("Effect component builds do not support watch mode")
      if (inputOptions.cache) throw new Error("Effect component builds do not support Rollup cache")
      started = true
    },
  }
  const finalPlugins = [
    staticContracts(sdkSource, publicEntries),
    makeSdkPlugin(),
    sharedEffectRuntime(input),
    ...finalCallerPlugins,
  ]
  return {
    ...finalOptions,
    input: entry,
    plugins: [
      lifecycle,
      {
        name: "golem-effect-static-exports",
        resolveId: (id) => (id === entry ? entry : null),
        load: (id) => (id === entry ? source : null),
        generateBundle() {
          this.emitFile({
            type: "asset",
            fileName: "capabilities.json",
            source: JSON.stringify(capabilities, null, 2) + "\n",
          })
        },
      },
      ...finalPlugins,
    ],
  }
}
