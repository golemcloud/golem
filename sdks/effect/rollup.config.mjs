import resolve from "@rollup/plugin-node-resolve"
import commonjs from "@rollup/plugin-commonjs"
import typescript from "rollup-plugin-typescript2"
import terser from "@rollup/plugin-terser"
import { defineConfig } from "rollup"

/**
 * Modules that are provided by the Golem host or by the WIT bindings
 * inside the base WASM. These must be left as external imports so the
 * component linker can satisfy them at instantiation time.
 *
 * `effect` is also externalized: the runtime is embedded in the base
 * WASM as a separate JS module (see scripts/generate-agent-template.mjs)
 * so that effect-golem and user components share a single Effect
 * runtime instance, avoiding identity issues caused by duplicate copies.
 */
const external = [
  "agent-guest",
  "golem:agent/common@1.5.0",
  "golem:agent/host@1.5.0",
  "golem:api/host@1.5.0",
  "golem:api/oplog@1.5.0",
  "golem:api/retry@1.5.0",
  "golem:core/types@1.5.0",
  "golem:quota/types@1.5.0",
  "wasi:cli/environment@0.2.3",
  "wasi:clocks/monotonic-clock@0.2.3",
  "wasi:clocks/wall-clock@0.2.3",
  "node:sqlite",
  "effect",
]

export default defineConfig([
  {
    input: "src/index.ts",
    output: {
      file: "dist/index.mjs",
      format: "esm",
      sourcemap: true,
    },
    external,
    // The Effect runtime is marked side-effect-free, which would otherwise
    // cause Rollup to drop our `export { Effect, Schema, Ref } from "effect"`
    // re-exports. Treat the entry module's re-exports as side-effectful so
    // user components embedded in the base WASM can resolve them.
    plugins: [
      resolve({ extensions: [".js", ".ts", ".mjs"] }),
      commonjs(),
      typescript({
        tsconfig: "./tsconfig.json",
        include: ["src/**/*", "golem-types/**/*"],
        tsconfigOverride: {
          compilerOptions: {
            declaration: false,
            sourceMap: true,
            module: "ESNext",
            moduleResolution: "Bundler",
          },
        },
      }),
      terser(),
    ],
  },

  // Standalone bundle of the `effect` runtime. Embedded as its own JS
  // module inside the base WASM so that effect-golem and user code share
  // one runtime instance.
  {
    input: "src/effect-bundle.mjs",
    output: {
      file: "dist/effect.mjs",
      format: "esm",
      sourcemap: false,
    },
    treeshake: false,
    plugins: [resolve({ extensions: [".mjs", ".js"] }), commonjs(), terser()],
  },
])
