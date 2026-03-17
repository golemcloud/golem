import componentRollupConfig from "../../golem-temp/common/ts/rollup.config.component.mjs";
import typescript from "@rollup/plugin-typescript";
import alias from "@rollup/plugin-alias";
import path from "node:path";
import url from "node:url";

// NOTE: this will be handled automatically soon with "simple templates"

let config = componentRollupConfig("benchmark-agent-ts");

config.plugins[config.plugins.length - 1] = typescript({
    noEmitOnError: true,
    include: [
        "./src/**/*.ts",
        "../../common-ts/**/*.ts",
    ],
});

const dir = path.dirname(url.fileURLToPath(import.meta.url));

export default config;
