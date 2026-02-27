import commonjs from "@rollup/plugin-commonjs";
import json from "@rollup/plugin-json";
import nodeResolve from "@rollup/plugin-node-resolve";
import typescript from "@rollup/plugin-typescript";
import url from "node:url";
import path from "node:path";
import process from "node:process";

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


    const dir = path.dirname(url.fileURLToPath(import.meta.url));

    const externalPackages = (id) => {
        return (
            id === "@golemcloud/golem-ts-sdk" ||
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
import { Metadata } from '${golemTemp}/ts-metadata/${componentName}/.metadata/generated-types';

TypescriptTypeRegistry.register(Metadata);

// Using an async function to prevent rollup from reordering registration and main import.
export default (async () => { return await import("./src/main");})();
`
                }
            }
        };
    }

    return {
        input: virtualAgentMainId,
        output: {
            file: `${golemTemp}/ts-dist/${componentName}/main.js`,
            format: "esm",
            inlineDynamicImports: true,
            sourcemap: false,
        },
        external: externalPackages,
        plugins: [
            virtualAgentMainPlugin(),
            nodeResolve({
                extensions: [".mjs", ".js", ".node", ".ts"],
            }),
            commonjs({
                include: [`${appRootDir}/node_modules/**`],
            }),
            json(),
            typescript({
                noEmitOnError: true,
                include: [
                    `./src/**/*.ts`,
                ],
            }),
        ],
    };
}

export default componentRollupConfig();
