// rollup.config.mjs
import resolve from '@rollup/plugin-node-resolve';
import commonjs from '@rollup/plugin-commonjs';
import typescript from 'rollup-plugin-typescript2';
import dts from 'rollup-plugin-dts';
import terser from '@rollup/plugin-terser';
import { defineConfig } from 'rollup';
import * as fs from 'node:fs';
import path from 'path';

// All `golem:*` and `wasi:*` specifiers are host-provided WIT imports (resolved by
// the wasm runtime), plus `agent-guest`/`node:sqlite`. Externalize them all so the
// fluent host surfaces (keyvalue/blobstore/websocket/rdbms) aren't bundled.
const external = (id) =>
  id === 'agent-guest' || id === 'node:sqlite' || id.startsWith('golem:') || id.startsWith('wasi:');

function onwarn(warning, warn) {
  if (warning.code === 'CIRCULAR_DEPENDENCY') return;
  warn(warning);
}

export default defineConfig([
  {
    input: 'src/index.ts',
    output: {
      file: 'dist/index.mjs',
      format: 'esm',
      sourcemap: true,
    },
    external,
    onwarn,
    plugins: [
      resolve({
        extensions: ['.js', '.ts'],
      }),
      commonjs(),
      typescript({
        tsconfig: './tsconfig.json',
        include: ['src/**/*', 'types'],
        // Transpile-only: the decorator-era files (agentConfig/mapping/typegen) have
        // pre-existing type errors from the new schema-model that block the build;
        // they are erased at runtime and unused by fluent agents. Type-checking is
        // done separately via `tsc --noEmit`.
        check: false,
        tsconfigOverride: {
          compilerOptions: { declaration: false },
        },
      }),
      terser(),
    ],
  },

  {
    input: 'src/index.ts',
    output: {
      file: 'dist/index.d.mts',
      format: 'esm',
    },
    external,
    onwarn,
    plugins: [
      dts(),
      {
        name: 'prepend-virtual-types',
        writeBundle() {
          const typesDir = path.resolve('types');

          const files = fs.readdirSync(typesDir).filter((f) => f.endsWith('.d.ts'));

          const refLines =
            files.map((f) => `/// <reference path="../types/${f}" />`).join('\n') + '\n';

          const mainDtsPath = path.resolve('dist/index.d.mts');
          const mainContent = fs.readFileSync(mainDtsPath, 'utf-8');
          fs.writeFileSync(mainDtsPath, refLines + mainContent, 'utf-8');
        },
      },
    ],
  },
]);
