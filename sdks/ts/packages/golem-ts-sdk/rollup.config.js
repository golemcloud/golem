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
// the wasm runtime), plus generated guest worlds and `node:sqlite`. Externalize them
// all so the SDK host surfaces (keyvalue/blobstore/websocket/rdbms) aren't bundled.
const external = (id) =>
  id === 'user' ||
  id === 'agent-guest' ||
  id === 'node:sqlite' ||
  id.startsWith('golem:') ||
  id.startsWith('wasi:');

function onwarn(warning, warn) {
  if (warning.code === 'CIRCULAR_DEPENDENCY') return;
  warn(warning);
}

function javascript(input, output, isExternal = external) {
  return {
    input,
    output: {
      file: output,
      format: 'esm',
      sourcemap: true,
    },
    external: isExternal,
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
  };
}

function prependVirtualTypes(output) {
  return {
    name: 'prepend-virtual-types',
    writeBundle() {
      const typesDir = path.resolve('types');
      const files = fs.readdirSync(typesDir).filter((file) => file.endsWith('.d.ts'));
      const refLines = files.map((file) => `/// <reference path="../types/${file}" />`).join('\n');
      const outputPath = path.resolve(output);
      const content = fs.readFileSync(outputPath, 'utf-8');
      fs.writeFileSync(outputPath, `${refLines}\n${content}`, 'utf-8');
    },
  };
}

function declarations(input, output) {
  return {
    input,
    output: {
      file: output,
      format: 'esm',
    },
    external,
    onwarn,
    plugins: [dts(), prependVirtualTypes(output)],
  };
}

export default (args) => defineConfig([
  {
    ...javascript('src/index.ts', 'dist/index.mjs'),
    external: (id) => external(id) || id.startsWith('@noble/hashes'),
    input: {
      index: 'src/index.ts',
      emptyGuest: 'src/emptyGuest.ts',
      middleware: 'src/middleware.ts',
      'schema/public': 'src/schema/public.ts',
      reflection: 'src/reflection.ts',
      toolClient: 'src/toolClient.ts',
      'internal/tool/compiled': 'src/internal/tool/compiled.ts',
      'internal/compiledAgent': 'src/internal/compiledAgent.ts',
    },
    output: {
      dir: 'dist/runtime',
      format: 'esm',
      preserveModules: true,
      preserveModulesRoot: 'src',
      entryFileNames: '[name].mjs',
    },
    plugins: javascript('src/index.ts', 'dist/index.mjs').plugins.slice(0, -1),
  },
  {
    ...javascript('src/index.ts', 'dist/index.mjs'),
    plugins: [
      ...javascript('src/index.ts', 'dist/index.mjs').plugins,
      {
        name: 'component-build',
        writeBundle() {
          fs.copyFileSync('scripts/component.mjs', 'dist/component.mjs');
          fs.copyFileSync('scripts/static-tools.mjs', 'dist/static-tools.mjs');
        },
      },
    ],
  },
  javascript('src/httpRouterContract.ts', 'dist/http-router.mjs'),
  javascript('src/wrapper.ts', 'dist/wrapper.mjs'),
  javascript('src/schema/public.ts', 'dist/schema.mjs'),
  javascript('src/reflection.ts', 'dist/reflection.mjs'),
  javascript('src/middleware.ts', 'dist/middleware.mjs'),
  javascript('src/middlewareRuntime.ts', 'dist/middleware-runtime.mjs'),
  declarations('src/index.ts', 'dist/index.d.mts'),
  declarations('src/httpRouterContract.ts', 'dist/http-router.d.mts'),
  declarations('src/schema/public.ts', 'dist/schema.d.mts'),
  declarations('src/reflection.ts', 'dist/reflection.d.mts'),
  declarations('src/middleware.ts', 'dist/middleware.d.mts'),
].filter((config) => !args.configHttpRouter || config.input === 'src/httpRouterContract.ts'));
