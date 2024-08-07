// @ts-check

import typescript from 'rollup-plugin-typescript2';
import resolve from "@rollup/plugin-node-resolve"

/**
 * @type {import('rollup').RollupOptions[]}
 */
export default [
  {
    input: "src/index.ts",
    output: {
      file: "dist/index.js",
      format: "esm",
    },
    external: [
      "wasi:cli/environment@0.2.0",
    ],
    plugins: [resolve(), typescript()],
  },
]
