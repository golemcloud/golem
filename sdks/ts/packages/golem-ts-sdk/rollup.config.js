// rollup.config.mjs
import resolve from '@rollup/plugin-node-resolve';
import commonjs from '@rollup/plugin-commonjs';
import typescript from 'rollup-plugin-typescript2';
import dts from 'rollup-plugin-dts';
import terser from '@rollup/plugin-terser';
import { defineConfig } from 'rollup';
import * as fs from 'node:fs';
import path from 'path';

const external = [
  'agent-guest',
  'golem:agent/common@1.5.0',
  'golem:agent/host@1.5.0',
  'golem:agent/common@2.0.0',
  'golem:agent/host@2.0.0',
  'golem:api/host@1.5.0',
  'golem:api/oplog@1.5.0',
  'golem:api/retry@1.5.0',
  'golem:core/types@1.5.0',
  'golem:core/types@2.0.0',
  'golem:quota/types@1.5.0',
  'golem:tool/common@0.1.0',
  'golem:secrets/types@0.1.0',
  'golem:secrets/reveal@0.1.0',
  'wasi:cli/environment@0.2.3',
  'wasi:clocks/monotonic-clock@0.2.3',
  'wasi:clocks/wall-clock@0.2.3',
  'wasi:io/streams@0.2.3',
  'node:sqlite',
];

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
