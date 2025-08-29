import alias from "@rollup/plugin-alias";
import json from "@rollup/plugin-json";
import nodeResolve from "@rollup/plugin-node-resolve";
import path from "node:path";
import typescript from "@rollup/plugin-typescript";
import url from "node:url";
import commonjs from "@rollup/plugin-commonjs";

export default function componentRollupConfig() {
    const dir = path.dirname(url.fileURLToPath(import.meta.url));

    const externalPackages = (id) => {
        return (
            id === "@golemcloud/golem-ts-sdk" ||
            id.startsWith("golem:api") ||
            id.startsWith("golem:rpc")
        );
    };

    return {
        input: ".agent/main.ts",
        output: {
            file: "dist/main.js",
            format: "esm",
            inlineDynamicImports: true,
            sourcemap: false,
        },
        external: externalPackages,
        plugins: [
            alias({
                entries: [
                    {
                        find: "common",
                        replacement: path.resolve(dir, "../common-ts/src"),
                    },
                ],
            }),
            nodeResolve({
                extensions: [".mjs", ".js", ".json", ".node", ".ts"],
            }),
            commonjs(),
            typescript({
                noEmitOnError: true,
                include: [
                    "./src/**/*.ts",
                    ".agent/**/*.ts",
                    "../../common-ts/src/**/*.ts",
                ],
            }),
            json(),
        ],
    };
}
