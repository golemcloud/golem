import commonjs from "@rollup/plugin-commonjs";
import json from "@rollup/plugin-json";
import nodeResolve from "@rollup/plugin-node-resolve";
import typescript from "@rollup/plugin-typescript";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

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

const embeddedPackages = new Set([
  "@golemcloud/effect-golem",
  "@golemcloud/effect-golem/sqlite",
  "@golemcloud/effect-golem/postgres",
  "@golemcloud/effect-golem/mysql",
  "@golemcloud/effect-golem/ignite2",
  "effect",
  "agent-guest",
]);

const externalPackages = (id) =>
  embeddedPackages.has(id) ||
  id.startsWith("golem:") ||
  id.startsWith("wasi:");

const require = createRequire(import.meta.url);
const effectPackageDir = path.dirname(
  require.resolve("effect/package.json", { paths: [appRootDir] }),
);
const expectedEffectVersion = "GOLEM_EFFECT_VERSION";
const actualEffectVersion = require(path.join(effectPackageDir, "package.json")).version;
if (actualEffectVersion !== expectedEffectVersion) {
  throw new Error(
    `effect@${actualEffectVersion} installed in this application does not match ` +
      `effect@${expectedEffectVersion} embedded in the Golem base WASM. ` +
      `Pin "effect" to "${expectedEffectVersion}" in package.json.`,
  );
}
const effectDistDir = path.join(effectPackageDir, "dist");
const effectRootFacadePrefix = "\0golem-effect-root-facade:";
const effectRedactedFacade = "\0golem-effect-redacted-facade";

const stableEffectModuleName = (source, importer) => {
  const packageSubpath = /^effect\/([^/]+)$/.exec(source);
  if (packageSubpath && /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(packageSubpath[1])) {
    const moduleName = packageSubpath[1];
    if (
      moduleName !== "index" &&
      existsSync(path.join(effectDistDir, `${moduleName}.js`))
    ) {
      return moduleName;
    }
    return undefined;
  }

  if (!importer || !source.startsWith(".") || importer.startsWith("\0")) {
    return undefined;
  }

  const resolved = path.resolve(path.dirname(importer), source);
  const relative = path.relative(effectDistDir, resolved);
  if (!relative.includes(path.sep) && relative.endsWith(".js")) {
    return path.basename(relative, ".js");
  }

  return undefined;
};

const resolvesToEffectInternal = (source, importer, internalPath) => {
  if (!importer || !source.startsWith(".") || importer.startsWith("\0")) {
    return false;
  }

  return (
    path.resolve(path.dirname(importer), source) ===
    path.join(effectDistDir, "internal", internalPath)
  );
};

const sharedEffectRuntime = () => ({
  name: "golem-shared-effect-runtime",
  resolveId(source, importer) {
    const moduleName = stableEffectModuleName(source, importer);
    if (moduleName) {
      return {
        id: `${effectRootFacadePrefix}${moduleName}`,
        moduleSideEffects: false,
      };
    }

    if (resolvesToEffectInternal(source, importer, "redacted.js")) {
      return { id: effectRedactedFacade, moduleSideEffects: false };
    }

    return null;
  },
  async load(id) {
    if (id === effectRedactedFacade) {
      return `
        import { Redacted } from "effect";
        export const value = Redacted.value;
        export const stringOrRedacted = (input) =>
          typeof input === "string" ? input : Redacted.value(input);
      `;
    }

    if (!id.startsWith(effectRootFacadePrefix)) {
      return null;
    }

    const moduleName = id.slice(effectRootFacadePrefix.length);
    const modulePath = path.join(effectDistDir, `${moduleName}.js`);
    let moduleExports;
    try {
      moduleExports = Object.keys(await import(pathToFileURL(modulePath)));
    } catch (error) {
      throw new Error(
        `Cannot share Effect module ${JSON.stringify(`effect/${moduleName}`)} ` +
          `with the embedded runtime: ${String(error)}`,
      );
    }

    const validExports = moduleExports.filter(
      (name) => name !== "default" && /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name),
    );
    return [
      `import { ${moduleName} as sharedModule } from "effect";`,
      ...validExports.map((name, index) => {
        const localName = `sharedExport${index}`;
        return `const ${localName} = /* @__PURE__ */ (() => sharedModule.${name})(); export { ${localName} as ${name} };`;
      }),
    ].join("\n");
  },
});

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
    sharedEffectRuntime(),
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
