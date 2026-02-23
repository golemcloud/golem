import alias from "@rollup/plugin-alias";
import commonjs from "@rollup/plugin-commonjs";
import json from "@rollup/plugin-json";
import nodeResolve from "@rollup/plugin-node-resolve";
import typescript from "@rollup/plugin-typescript";
import url from "node:url";
import path from "node:path";

export default function componentRollupConfig(componentName) {
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
import { Metadata } from '../../golem-temp/ts-metadata/${componentName}/.metadata/generated-types';

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
            file: `../../golem-temp/ts-dist/${componentName}/main.js`,
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
                include: ["../../node_modules/**"],
            }),
            json(),
            typescript({
                noEmitOnError: true,
                include: [
                    "./src/**/*.ts",
                    ".agent/**/*.ts",
                    ".metadata/**/*.ts",
                    "../../common-ts/src/**/*.ts",
                ],
            }),
        ],
    };
}
