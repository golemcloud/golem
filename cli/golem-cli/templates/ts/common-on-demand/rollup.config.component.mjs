import commonjs from "@rollup/plugin-commonjs";
import json from "@rollup/plugin-json";
import nodeResolve from "@rollup/plugin-node-resolve";
import typescript from "@rollup/plugin-typescript";
import ts from "typescript";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

// Rollup config for a fluent (ts) agent component.
//
// The component's tsconfig.json is the single source of truth: it decides which
// files are compiled (`include`/`files`) and how module path aliases resolve
// (`compilerOptions.paths`). This config reads it once and defers to
// @rollup/plugin-typescript rather than restating any of it here, so the build
// and the type checker always agree on the same file set and resolution rules.
//
// The fluent SDK derives agent metadata at runtime from the schemas, so the
// virtual entry only imports the user's main module for its side-effecting
// `defineAgent(...).implement(...)` registrations. `@golemcloud/golem-ts-sdk` and
// the `golem:*` host packages are externalized (provided by the prebuilt
// agent_guest.wasm); user code and the schema library are bundled into main.js
// and injected into that wasm.

// Read tsconfig.json through the TypeScript compiler API — the same path
// @rollup/plugin-typescript takes — so comments and `extends` are honored, and a
// missing or invalid tsconfig fails the build with a clear error instead of being
// ignored and producing a confusing failure further down the pipeline.
function loadComponentTsConfig(componentDir) {
    const tsconfigPath = path.join(componentDir, "tsconfig.json");
    if (!fs.existsSync(tsconfigPath)) {
        throw new Error(`tsconfig.json not found at ${tsconfigPath}`);
    }

    const { config, error } = ts.readConfigFile(tsconfigPath, ts.sys.readFile);
    if (error) {
        throw new Error(
            `Failed to read ${tsconfigPath}: ${ts.flattenDiagnosticMessageText(error.messageText, "\n")}`,
        );
    }

    const parsed = ts.parseJsonConfigFileContent(config, ts.sys, componentDir);
    const errors = parsed.errors.filter((d) => d.category === ts.DiagnosticCategory.Error);
    if (errors.length > 0) {
        const message = errors
            .map((d) => ts.flattenDiagnosticMessageText(d.messageText, "\n"))
            .join("\n");
        throw new Error(`Invalid ${tsconfigPath}:\n${message}`);
    }

    return parsed;
}

function componentRollupConfig() {
    const componentName = process.env.GOLEM_COMPONENT_NAME;
    if (!componentName) {
        throw new Error("GOLEM_COMPONENT_NAME is not set");
    }
    const golemTemp = process.env.GOLEM_TEMP;
    if (!golemTemp) {
        throw new Error("GOLEM_TEMP is not set");
    }

    const componentDir = process.cwd();
    const parsedTsConfig = loadComponentTsConfig(componentDir);

    // Compile exactly the files the tsconfig resolves. `parsed.fileNames` is
    // TypeScript's own expansion of `include`/`files`/`exclude`, which excludes
    // node_modules by default. Scoping the plugin to this set keeps dependencies
    // out of TypeScript compilation: a package that ships `.ts` sources next to
    // its compiled `.js` would otherwise be dragged in through TypeScript's
    // `.js`->`.ts` source redirect, and rollup would fail parsing that `.ts` as
    // plain JavaScript. Path aliases (`compilerOptions.paths`) are resolved by the
    // plugin itself (`ts.resolveModuleName`), so no separate alias plugin is
    // needed. Fall back to the conventional `src/` glob only when the tsconfig
    // resolves no files.
    const include = parsedTsConfig.fileNames.length > 0
        ? parsedTsConfig.fileNames
        : ["./src/**/*.ts"];

    const externalPackages = (id) =>
        id === "@golemcloud/golem-ts-sdk" || id.startsWith("golem:");

    const virtualAgentMainId = "virtual:agent-main";
    const resolvedVirtualAgentMainId = "\0virtual:agent-main";
    const virtualAgentMainPlugin = () => ({
        name: "agent-main",
        resolveId(id) {
            if (id === virtualAgentMainId) {
                return resolvedVirtualAgentMainId;
            }
        },
        load(id) {
            if (id === resolvedVirtualAgentMainId) {
                // Async wrapper keeps rollup from reordering the side-effecting import.
                return `export default (async () => { return await import("./src/main"); })();`;
            }
        },
    });

    const plugins = [
        virtualAgentMainPlugin(),
        nodeResolve({ extensions: [".mjs", ".js", ".node", ".ts"] }),
        commonjs(),
        json(),
        typescript({
            noEmitOnError: true,
            include,
        }),
    ];

    return {
        input: virtualAgentMainId,
        output: {
            file: path.join(golemTemp, "ts-dist", componentName, "main.js"),
            format: "esm",
            inlineDynamicImports: true,
            sourcemap: false,
        },
        external: externalPackages,
        plugins,
    };
}

export default componentRollupConfig();
