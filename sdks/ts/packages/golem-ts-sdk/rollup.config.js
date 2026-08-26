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
// SDK host surfaces (keyvalue/blobstore/websocket/rdbms) aren't bundled.
const external = (id) =>
  id === 'agent-guest' || id === 'node:sqlite' || id.startsWith('golem:') || id.startsWith('wasi:');

function onwarn(warning, warn) {
  if (warning.code === 'CIRCULAR_DEPENDENCY') return;
  warn(warning);
}

const entries = [
  { input: 'src/index.ts', name: 'index' },
  { input: 'src/schema/public.ts', name: 'schema' },
  { input: 'src/reflection.ts', name: 'reflection' },
];

export default defineConfig([
  ...entries.map(({ input, name }) => ({
    input,
    output: {
      file: `dist/${name}.mjs`,
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
        tsconfigOverride: {
          compilerOptions: { declaration: false },
        },
      }),
      terser(),
    ],
  })),
  ...entries.map(({ input, name }) => ({
    input,
    output: {
      file: `dist/${name}.d.mts`,
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

          const mainDtsPath = path.resolve(`dist/${name}.d.mts`);
          const mainContent = fs.readFileSync(mainDtsPath, 'utf-8');
          fs.writeFileSync(mainDtsPath, refLines + mainContent, 'utf-8');
        },
      },
    ],
  })),
]);
