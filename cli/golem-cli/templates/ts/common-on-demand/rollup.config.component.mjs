import alias from "@rollup/plugin-alias";
import commonjs from "@rollup/plugin-commonjs";
import json from "@rollup/plugin-json";
import nodeResolve from "@rollup/plugin-node-resolve";
import typescript from "@rollup/plugin-typescript";
import url from "node:url";
import path from "node:path";
import process from "node:process";
import fs from "node:fs";

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

            // Convert tsconfig path pattern like "common/*" -> find: "common"
            const find = key.replace(/\/\*$/, "");
            // Convert target like "../common-ts/src/*" -> replacement directory
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

    const appRootDir = process.env.GOLEM_APP_ROOT;
    if (!appRootDir) {
        throw new Error("GOLEM_APP_ROOT is not set");
    }

    const componentDir = process.cwd();
    const metadataModulePath = path.relative(
        componentDir,
        path.resolve(golemTemp, 'ts-metadata', componentName, '.metadata', 'generated-types.ts')
    ).split(path.sep).join('/');

    const { aliasEntries, tsIncludes } = readTsConfigPaths(componentDir);

    const externalPackages = (id) => {
        return (
          id === "@golemcloud/golem-ts-sdk" ||
            id === "@golemcloud/golem-ts-types-core" ||
            id.startsWith("golem:")
        );
    };

    const virtualAgentMainId = "virtual:agent-main";
    const resolvedVirtualAgentMainId = "\0virtual:agent-main";

    const virtualAgentMainPlugin = () => {
        return {
            name: "agent-main",
            resolveId(id) {
                if (id === virtualAgentMainId) {
                    return resolvedVirtualAgentMainId;
                }
            },
            load(id) {
                if (id === resolvedVirtualAgentMainId) {
                    return `
import { TypescriptTypeRegistry } from '@golemcloud/golem-ts-sdk';
import { Metadata } from './${metadataModulePath}';

TypescriptTypeRegistry.register(Metadata);

// Using an async function to prevent rollup from reordering registration and main import.
export default (async () => { return await import("./src/main");})();
`
                }
            }
        };
    }

    const plugins = [
        virtualAgentMainPlugin(),
    ];

    if (aliasEntries.length > 0) {
        plugins.push(alias({ entries: aliasEntries }));
    }

    plugins.push(
        nodeResolve({
            extensions: [".mjs", ".js", ".node", ".ts"],
        }),
        commonjs({
            include: [`${appRootDir}/node_modules/**`],
        }),
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
            file: `${golemTemp}/ts-dist/${componentName}/main.js`,
            format: "esm",
            inlineDynamicImports: true,
            sourcemap: false,
        },
        external: externalPackages,
        plugins,
    };
}

export default componentRollupConfig();
