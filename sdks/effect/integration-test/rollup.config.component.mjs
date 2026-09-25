/**
 * Rollup config used by the `effect-golem-ts` component template defined
 * in `golem.yaml`. SDK capabilities are bundled with the application;
 * Effect and host modules resolve from the base WASM.
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
import { rollup } from "rollup"
import { componentConfiguration } from "@golemcloud/effect-golem/build"

const componentName = process.env.GOLEM_COMPONENT_NAME
const golemTemp = process.env.GOLEM_TEMP
const appRootDir = process.env.GOLEM_APP_ROOT

if (!componentName) throw new Error("GOLEM_COMPONENT_NAME env var is not set")
if (!golemTemp) throw new Error("GOLEM_TEMP env var is not set")
if (!appRootDir) throw new Error("GOLEM_APP_ROOT env var is not set")

const externalPackages = (id) =>
  id === "node:sqlite" ||
  id === "effect" ||
  id === "effect/unstable/http" ||
  id.startsWith("golem:") ||
  id.startsWith("wasi:") ||
  id === "agent-guest"

export default await componentConfiguration(rollup, {
  input: process.env.GOLEM_COMPONENT_ENTRY ?? "./src/main.ts",
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
})
