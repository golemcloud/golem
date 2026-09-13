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
const external = (id) =>
  id === "agent-guest" ||
  id === "tool-middleware-guest" ||
  id === "agent-tool-middleware-guest" ||
  id === "node:sqlite" ||
  id === "effect" ||
  id === "@golemcloud/effect-golem" ||
  id.startsWith("@golemcloud/effect-golem/") ||
  id.startsWith("golem:") ||
  id.startsWith("wasi:")

function assertMiddlewareHostNeutral() {
  return {
    name: "assert-middleware-host-neutral",
    generateBundle(_options, bundle) {
      for (const output of Object.values(bundle)) {
        if (output.type !== "chunk") continue
        const forbidden = [...output.imports, ...output.dynamicImports].filter(
          (id) => id === "golem:tool/host@0.1.0" || id === "node:sqlite",
        )
        if (forbidden.length > 0) {
          this.error(`Middleware bundle reached agent-only hosts:\n${forbidden.join("\n")}`)
        }
      }
    },
  }
}

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

  {
    input: "src/Middleware.ts",
    output: {
      file: "dist/middleware.mjs",
      format: "esm",
      sourcemap: true,
    },
    external,
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
      assertMiddlewareHostNeutral(),
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

  // SqliteClient adapter. Externalizes `effect`, `effect-golem`, and
  // `node:sqlite` so the resulting bundle is small and shares the
  // single Effect runtime instance embedded into the base WASM.
  {
    input: "src/Sqlite/SqliteClient.ts",
    output: {
      file: "dist/sqlite.mjs",
      format: "esm",
      sourcemap: true,
    },
    external,
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

  // PgClient adapter. Externalizes `effect`, `effect-golem`, and the
  // `golem:rdbms/*` host bindings so the bundle stays small and shares
  // the single Effect runtime instance embedded into the base WASM.
  {
    input: "src/Postgres/PgClient.ts",
    output: {
      file: "dist/postgres.mjs",
      format: "esm",
      sourcemap: true,
    },
    external,
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

  // MySqlClient adapter. Same externalization as PgClient.
  {
    input: "src/Mysql/MySqlClient.ts",
    output: {
      file: "dist/mysql.mjs",
      format: "esm",
      sourcemap: true,
    },
    external,
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

  // IgniteClient adapter. Same externalization. Deployed as an
  // optional adapter — some Golem environments may not expose
  // `golem:rdbms/ignite2@1.5.0`.
  {
    input: "src/Ignite/IgniteClient.ts",
    output: {
      file: "dist/ignite.mjs",
      format: "esm",
      sourcemap: true,
    },
    external,
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
])
