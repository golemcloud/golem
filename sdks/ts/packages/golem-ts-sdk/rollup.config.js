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
  id === 'agent-guest' ||
  id === 'tool-middleware-guest' ||
  id === 'node:sqlite' ||
  id.startsWith('golem:') ||
  id.startsWith('wasi:');

function onwarn(warning, warn) {
  if (warning.code === 'CIRCULAR_DEPENDENCY') return;
  warn(warning);
}

function assertHostNeutralBundle() {
  return {
    name: 'assert-host-neutral-bundle',
    generateBundle(_options, bundle) {
      for (const output of Object.values(bundle)) {
        if (output.type !== 'chunk') continue;
        const forbiddenImports = [...output.imports, ...output.dynamicImports].filter((id) =>
          id.startsWith('golem:tool/host'),
        );
        const forbiddenModules = Object.keys(output.modules).filter((id) => {
          const normalized = id.replaceAll('\\', '/');
          return (
            normalized.endsWith('/src/bridge/tool.ts') || normalized.endsWith('/src/toolClient.ts')
          );
        });
        if (forbiddenImports.length > 0 || forbiddenModules.length > 0) {
          this.error(
            `Host-neutral middleware bundle reached the ambient tool host:\n${[
              ...forbiddenImports,
              ...forbiddenModules,
            ].join('\n')}`,
          );
        }
      }
    },
  };
}

function javascript(input, output, { hostNeutral = false } = {}) {
  return {
    input,
    output: {
      file: output,
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
      ...(hostNeutral ? [assertHostNeutralBundle()] : []),
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

export default defineConfig([
  javascript('src/index.ts', 'dist/index.mjs'),
  javascript('src/middleware.ts', 'dist/middleware.mjs', { hostNeutral: true }),
  javascript('src/middlewareRuntime.ts', 'dist/middleware-runtime.mjs', { hostNeutral: true }),
  declarations('src/index.ts', 'dist/index.d.mts'),
  declarations('src/middleware.ts', 'dist/middleware.d.mts'),
]);
