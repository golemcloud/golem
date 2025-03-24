import * as fs from "node:fs";
import alias from '@rollup/plugin-alias';
import nodeResolve from "@rollup/plugin-node-resolve";
import path from "node:path";
import url from "node:url";

export default function componentRollupConfig() {
    const dir = path.dirname(url.fileURLToPath(import.meta.url));
    const moduleRegex = /declare\s+module\s+"([^"]+)"/g;
    const generated_interfaces_dir = "src/generated/interfaces";

    const externalPackages = (() => {
        if (!fs.existsSync(generated_interfaces_dir)) {
            return [];
        }
        return fs
            .readdirSync(generated_interfaces_dir, {withFileTypes: true})
            .filter(dirent => dirent.isFile() && dirent.name.endsWith(".d.ts"))
            .flatMap(dirent =>
                [...fs.readFileSync(path.join(dirent.parentPath, dirent.name))
                    .toString()
                    .matchAll(moduleRegex)]
                    .map((match) => {
                        const moduleName = match[1];
                        if (moduleName === undefined) {
                            throw new Error(`Missing match for module name`);
                        }
                        return moduleName;
                    }),
            );
    })();

    console.log("External packages:", externalPackages);

    return {
        input: "src/main.js",
        output: {
            file: "dist/main.js",
            format: "esm",
        },
        external: externalPackages,
        plugins: [
            alias({
                entries: [
                    {find: 'common', replacement: path.resolve(dir, "../common-js/src")}
                ]
            }),
            nodeResolve({
                extensions: [".mjs", ".js", ".json", ".node", ".ts"]
            })
        ],
    };
}


