/**
 * Rollup config used by the `effect-golem-ts` component template defined
 * in `golem.yaml`. Adapted from the official TS SDK template's
 * `rollup.config.component.mjs`, with these differences:
 *
 *   - Externalizes `effect-golem` (instead of `@golemcloud/golem-ts-sdk`)
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

const componentName = process.env.GOLEM_COMPONENT_NAME
const golemTemp = process.env.GOLEM_TEMP
const appRootDir = process.env.GOLEM_APP_ROOT

if (!componentName) throw new Error("GOLEM_COMPONENT_NAME env var is not set")
if (!golemTemp) throw new Error("GOLEM_TEMP env var is not set")
if (!appRootDir) throw new Error("GOLEM_APP_ROOT env var is not set")

const externalPackages = (id) =>
  id === "effect-golem" ||
  id === "effect" ||
  id.startsWith("golem:") ||
  id.startsWith("wasi:") ||
  id === "agent-guest"

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
    nodeResolve({ extensions: [".mjs", ".js", ".node", ".ts"] }),
    commonjs({ include: [`${appRootDir}/node_modules/**`] }),
    json(),
    typescript({ noEmitOnError: true }),
  ],
}
