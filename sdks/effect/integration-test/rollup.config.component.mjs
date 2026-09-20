/**
 * Rollup config used by the `effect-golem-ts` component template defined
 * in `golem.yaml`. Adapted from the official TS SDK template's
 * `rollup.config.component.mjs`, with these differences:
 *
 *   - Externalizes `@golemcloud/effect-golem` (instead of `@golemcloud/golem-ts-sdk`)
 *     and all `golem:*` / `wasi:*` / `agent-guest` host module IDs. The
 *     resolved values come from the prebuilt base WASM at runtime.
 *   - Does not depend on `golem-typegen`-generated metadata: agents
 *     defined via effect-golem's `defineAgent(...)` are self-describing.
 *   - Bundles `effect` into the user code (the base WASM also embeds its
 *     own copy via `effect-golem`'s bundle).
 *
 * Invoked by the build pipeline with these env vars set:
 *   GOLEM_APP_ROOT       — the integration-test/ directory
 *   GOLEM_TEMP           — the per-build golem-temp/ directory
 *   GOLEM_COMPONENT_NAME — kebab-case component name
 */
import commonjs from "@rollup/plugin-commonjs"
import json from "@rollup/plugin-json"
import nodeResolve from "@rollup/plugin-node-resolve"
import typescript from "@rollup/plugin-typescript"
import process from "node:process"
import { externalPackages } from "./component-bundle-policy.mjs"
import { createRequire } from "node:module"
import path from "node:path"
import { pathToFileURL } from "node:url"

const componentName = process.env.GOLEM_COMPONENT_NAME
const golemTemp = process.env.GOLEM_TEMP
const appRootDir = process.env.GOLEM_APP_ROOT

if (!componentName) throw new Error("GOLEM_COMPONENT_NAME env var is not set")
if (!golemTemp) throw new Error("GOLEM_TEMP env var is not set")
if (!appRootDir) throw new Error("GOLEM_APP_ROOT env var is not set")

const effectDist = path.join(
  path.dirname(createRequire(import.meta.url).resolve("effect/package.json")),
  "dist",
)
const httpFacade = "\0golem-http:"
const sharedHttpRuntime = {
  name: "golem-shared-http-runtime",
  resolveId(source, importer) {
    const relative =
      importer && source.startsWith(".") && !importer.startsWith("\0")
        ? path
            .relative(effectDist, path.resolve(path.dirname(importer), source))
            .split(path.sep)
            .join("/")
            .replace(/\.js$/, "")
        : source.replace(/^effect\//, "")
    return /^unstable\/(http|httpapi)\/[A-Za-z_$][A-Za-z0-9_$]*$/.test(relative)
      ? `${httpFacade}${relative}`
      : null
  },
  async load(id) {
    if (!id.startsWith(httpFacade)) return null
    const subpath = id.slice(httpFacade.length)
    const separator = subpath.lastIndexOf("/")
    const namespace = subpath.slice(separator + 1)
    const barrel = `effect/${subpath.slice(0, separator)}`
    const names = Object.keys(await import(pathToFileURL(path.join(effectDist, `${subpath}.js`))))
    return (
      `import { ${namespace} as shared } from ${JSON.stringify(barrel)};\n` +
      names.map((name) => `export const ${name} = shared.${name};`).join("\n")
    )
  },
}

export default {
  input: "./src/main.ts",
  output: {
    file: `${golemTemp}/ts-dist/${componentName}/main.js`,
    format: "esm",
    inlineDynamicImports: true,
    sourcemap: false,
  },
  external: externalPackages,
  plugins: [
    sharedHttpRuntime,
    nodeResolve({ extensions: [".mjs", ".js", ".node", ".ts"] }),
    commonjs({ include: [`${appRootDir}/node_modules/**`] }),
    json(),
    typescript({ noEmitOnError: true }),
  ],
}
