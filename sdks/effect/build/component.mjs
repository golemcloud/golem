import { dirname, join, resolve, sep } from "node:path"
import { fileURLToPath } from "node:url"
import { sharedEffectRuntime } from "./shared-effect.mjs"
import { staticContracts } from "./static-contracts.mjs"

const sdkSource = resolve(dirname(fileURLToPath(import.meta.url)), "../dist/src")
const entry = "\0golem-effect-component"
const packageName = "@golemcloud/effect-golem"
const publicEntries = new Map([
  [packageName, "index.js"],
  [`${packageName}/middleware`, "Middleware.js"],
  [`${packageName}/sqlite`, "Sqlite/SqliteClient.js"],
  [`${packageName}/postgres`, "Postgres/PgClient.js"],
  [`${packageName}/mysql`, "Mysql/MySqlClient.js"],
  [`${packageName}/ignite2`, "Ignite/IgniteClient.js"],
])

/**
 * Build-time capability selection, based on the tree-shaken application graph.
 * Discovery does not execute application code. A retained definition module is
 * conservatively considered capable even if registration is conditional.
 */
export async function componentConfiguration(rollup, options) {
  if (typeof options.input !== "string") throw new Error("Expected one component entrypoint")
  const input = resolve(options.input)
  const sdk = {
    name: "golem-effect-sdk-source",
    resolveId(source) {
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
  const plugins = [
    staticContracts(sdkSource, publicEntries),
    sdk,
    sharedEffectRuntime(input),
    ...(options.plugins ?? []),
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
  return {
    ...options,
    input: entry,
    plugins: [
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
      ...plugins,
    ],
  }
}
