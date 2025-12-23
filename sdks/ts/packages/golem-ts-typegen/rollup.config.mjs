import resolve from '@rollup/plugin-node-resolve';
import commonjs from '@rollup/plugin-commonjs';
import typescript from 'rollup-plugin-typescript2';
import dts from 'rollup-plugin-dts';
import { defineConfig } from 'rollup';

const external = [];

export default defineConfig([
    {
        input: 'src/index.ts',
        output: {
            file: 'dist/index.mjs',
            format: 'esm',
            sourcemap: true,
        },
        external,
        plugins: [
            resolve({ extensions: ['.js', '.ts'] }),
            commonjs(),
            typescript({
                tsconfig: './tsconfig.json',
                useTsconfigDeclarationDir: true,
                tsconfigOverride: { compilerOptions: { declaration: true, declarationDir: 'dist' } }
            }),
        ],
    },
    {
        input: 'src/index.ts',
        output: { file: 'dist/index.d.mts', format: 'esm' },
        external,
        plugins: [dts()],
    }
]);
