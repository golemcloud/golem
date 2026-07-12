import alias from "@rollup/plugin-alias";
import commonjs from "@rollup/plugin-commonjs";
import json from "@rollup/plugin-json";
import nodeResolve from "@rollup/plugin-node-resolve";
import typescript from "@rollup/plugin-typescript";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

// Rollup config for the fluent (ts) component template. Unlike the
// decorator `ts` template there is NO typegen step: the fluent SDK derives agent
// metadata at runtime from the schemas, so the virtual entry simply imports the
// user's main module for its `defineAgent(...).implement(...)` registration side
// effects. `@golemcloud/golem-ts-sdk` and the `golem:*` host packages are
// externalized (provided by the prebuilt agent_guest.wasm); the schema library
// and user code are bundled into main.js and injected into that wasm.

function readTsConfigPaths(componentDir) {
    const tsconfigPath = path.join(componentDir, "tsconfig.json");
    if (!fs.existsSync(tsconfigPath)) {
        return { aliasEntries: [], tsIncludes: [] };
    }

    try {
        const tsconfig = JSON.parse(fs.readFileSync(tsconfigPath, "utf-8"));
        const paths = tsconfig?.compilerOptions?.paths;
        if (!paths) {
            return { aliasEntries: [], tsIncludes: [] };
        }

        const aliasEntries = [];
        const tsIncludes = [];

        for (const [key, values] of Object.entries(paths)) {
            if (!values || values.length === 0) continue;

            const find = key.replace(/\/\*$/, "");
            const replacement = path.resolve(componentDir, values[0].replace(/\/\*$/, ""));

            aliasEntries.push({ find, replacement });
            tsIncludes.push(path.resolve(componentDir, values[0].replace(/\*$/, "**/*.ts")));
        }

        return { aliasEntries, tsIncludes };
    } catch {
        return { aliasEntries: [], tsIncludes: [] };
    }
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
    const { aliasEntries, tsIncludes } = readTsConfigPaths(componentDir);

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

    const plugins = [virtualAgentMainPlugin()];

    if (aliasEntries.length > 0) {
        plugins.push(alias({ entries: aliasEntries }));
    }

    plugins.push(
        nodeResolve({ extensions: [".mjs", ".js", ".node", ".ts"] }),
        commonjs(),
        json(),
        typescript({
            noEmitOnError: true,
            ...(tsIncludes.length > 0
                ? { include: ["./src/**/*.ts", ...tsIncludes] }
                : {}),
        }),
    );

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
