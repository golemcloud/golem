import commonjs from "@rollup/plugin-commonjs";
import json from "@rollup/plugin-json";
import nodeResolve from "@rollup/plugin-node-resolve";
import typescript from "@rollup/plugin-typescript";
import ts from "typescript";
import { createRequire } from "node:module";
import path from "node:path";
import process from "node:process";
import { rollup } from "rollup";
import { componentConfiguration } from "@golemcloud/effect-golem/build";

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

const embeddedPackages = new Set(["effect", "agent-guest"]);

const externalPackages = (id) =>
  embeddedPackages.has(id) ||
  id === "node:sqlite" ||
  id.startsWith("golem:") ||
  id.startsWith("wasi:");

const tsconfigPath = path.resolve("tsconfig.json");
const { config, error } = ts.readConfigFile(tsconfigPath, ts.sys.readFile);
if (error) {
  throw new Error(ts.flattenDiagnosticMessageText(error.messageText, "\n"));
}
const parsedTsConfig = ts.parseJsonConfigFileContent(
  config,
  ts.sys,
  process.cwd(),
);
if (parsedTsConfig.errors.length > 0) {
  throw new Error(
    parsedTsConfig.errors
      .map((error) => ts.flattenDiagnosticMessageText(error.messageText, "\n"))
      .join("\n"),
  );
}

const require = createRequire(import.meta.url);
const effectPackageDir = path.dirname(
  require.resolve("effect/package.json", { paths: [appRootDir] }),
);
const expectedEffectVersion = "GOLEM_EFFECT_VERSION";
const actualEffectVersion = require(
  path.join(effectPackageDir, "package.json"),
).version;
if (actualEffectVersion !== expectedEffectVersion) {
  throw new Error(
    `effect@${actualEffectVersion} installed in this application does not match ` +
      `effect@${expectedEffectVersion} embedded in the Golem base WASM. ` +
      `Pin "effect" to "${expectedEffectVersion}" in package.json.`,
  );
}
const configuration = {
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
      include: parsedTsConfig.fileNames,
    }),
  ],
};

export default await componentConfiguration(rollup, configuration);
