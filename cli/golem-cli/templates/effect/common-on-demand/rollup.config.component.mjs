import commonjs from "@rollup/plugin-commonjs";
import json from "@rollup/plugin-json";
import nodeResolve from "@rollup/plugin-node-resolve";
import typescript from "@rollup/plugin-typescript";
import process from "node:process";

const componentName = process.env.GOLEM_COMPONENT_NAME;
const golemTemp = process.env.GOLEM_TEMP;
const appRootDir = process.env.GOLEM_APP_ROOT;

if (!componentName) {
  throw new Error("GOLEM_COMPONENT_NAME is not set");
}
if (!golemTemp) {
  throw new Error("GOLEM_TEMP is not set");
}
if (!appRootDir) {
  throw new Error("GOLEM_APP_ROOT is not set");
}

const externalPackages = (id) =>
  id === "@golemcloud/effect-golem" ||
  id === "@golemcloud/effect-golem/sqlite" ||
  id === "@golemcloud/effect-golem/postgres" ||
  id === "@golemcloud/effect-golem/mysql" ||
  id === "@golemcloud/effect-golem/ignite2" ||
  id === "effect" ||
  id === "node:sqlite" ||
  id === "agent-guest" ||
  id.startsWith("golem:") ||
  id.startsWith("wasi:");

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
    nodeResolve({
      extensions: [".mjs", ".js", ".node", ".ts"],
    }),
    commonjs({
      include: [`${appRootDir}/node_modules/**`],
    }),
    json(),
    typescript({
      noEmitOnError: true,
    }),
  ],
};
