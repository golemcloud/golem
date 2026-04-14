// rollup.config.mjs
import typescript from 'rollup-plugin-typescript2';
import dts from 'rollup-plugin-dts';
import { defineConfig } from 'rollup';

export default defineConfig([
  {
    input: 'src/index.ts',
    output: {
      file: 'dist/index.mjs',
      format: 'esm',
      sourcemap: true,
    },
    external: [
      'node:child_process',
      'node:fs',
      'node:process',
      'node:repl',
      'node:stream',
      'node:util',
      'picocolors',
      'ts-morph',
      'uuid',
    ],
    plugins: [
      typescript({
        tsconfig: './tsconfig.json',
        useTsconfigDeclarationDir: true,
        tsconfigOverride: {
          compilerOptions: { declaration: false },
        },
      }),
    ],
  },

  {
    input: 'src/index.ts',
    output: {
      file: 'dist/index.d.mts',
      format: 'esm',
    },
    plugins: [dts()],
  },
]);
