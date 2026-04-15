import resolve from '@rollup/plugin-node-resolve';
import commonjs from '@rollup/plugin-commonjs';
import typescript from 'rollup-plugin-typescript2';
import { defineConfig } from 'rollup';

export default defineConfig([
  {
    input: 'src/bin/golem-typegen.ts',
    output: {
      file: 'dist/golem-typegen.cjs',
      format: 'cjs',
    },
    plugins: [
      resolve({ extensions: ['.js', '.ts'] }),
      commonjs(),
      typescript({
        tsconfig: './tsconfig.json',
        tsconfigOverride: {
          compilerOptions: { declaration: false },
        },
      }),
    ],
  },
]);
