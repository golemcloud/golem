import commonjs from '@rollup/plugin-commonjs';
import json from '@rollup/plugin-json';
import nodeResolve from '@rollup/plugin-node-resolve';
import fs from 'node:fs';
import path from 'node:path';
import { applyGuardedBuildPatch, npmBuildPatches } from './build-patches.mjs';

const npmRoot = 'node_modules/npm';
const absoluteNpmRoot = path.resolve(npmRoot);
const typescriptRoot = 'node_modules/typescript';
const absoluteTypescriptRoot = path.resolve(typescriptRoot);
const inspectorFacade = path.resolve('compat/inspector/index.js');

const normalizeLegacyEscape = {
  name: 'normalize-upstream-sloppy-mode-syntax',
  load(id) {
    if (!id.startsWith(`${absoluteNpmRoot}/`) || !id.endsWith('.js')) return null;
    let source = fs.readFileSync(id, 'utf8');
    const relative = path.relative(absoluteNpmRoot, id).split(path.sep).join('/');
    const patch = npmBuildPatches.get(relative);
    if (patch) source = applyGuardedBuildPatch(patch, source);
    const runtimeFile = `/toolchain/npm/node_modules/npm/${relative}`;
    const runtimeDirectory = runtimeFile.slice(0, runtimeFile.lastIndexOf('/'));
    return source
      .replace(/(?<![.$\w])__filename\b/g, JSON.stringify(runtimeFile))
      .replace(/(?<![.$\w])__dirname\b/g, JSON.stringify(runtimeDirectory));
  },
};

const preserveTypescriptRuntimePaths = {
  name: 'preserve-typescript-runtime-paths',
  load(id) {
    if (!id.startsWith(`${absoluteTypescriptRoot}/`) || !id.endsWith('.js')) return null;
    const relative = path.relative(absoluteTypescriptRoot, id).split(path.sep).join('/');
    const runtimeFile = `/toolchain/typescript/node_modules/typescript/${relative}`;
    const runtimeDirectory = runtimeFile.slice(0, runtimeFile.lastIndexOf('/'));
    return fs
      .readFileSync(id, 'utf8')
      .replace(/(?<![.$\w])__filename\b/g, JSON.stringify(runtimeFile))
      .replace(/(?<![.$\w])__dirname\b/g, JSON.stringify(runtimeDirectory));
  },
};

const provideInspectorFacade = {
  name: 'provide-typescript-inspector-facade',
  resolveId(id) {
    return id === 'inspector' || id === 'node:inspector' ? inspectorFacade : null;
  },
};

export default [
  {
    input: 'bundle-entries/npm-cli.mjs',
    output: { file: 'generated/npm-cli.cjs', format: 'cjs', inlineDynamicImports: true },
    external: (id) => id.startsWith('node:'),
    plugins: [
      normalizeLegacyEscape,
      nodeResolve({ preferBuiltins: true, exportConditions: ['node'] }),
      commonjs({
        dynamicRequireTargets: [
          `${npmRoot}/lib/cli/entry.js`,
          `${npmRoot}/lib/commands/*.js`,
          `${npmRoot}/node_modules/node-gyp/bin/node-gyp.js`,
        ],
        ignoreDynamicRequires: false,
      }),
      json(),
    ],
  },
  {
    input: 'bundle-entries/tsc.mjs',
    output: { file: 'generated/tsc.cjs', format: 'cjs', inlineDynamicImports: true },
    external: (id) => id.startsWith('node:'),
    plugins: [
      preserveTypescriptRuntimePaths,
      provideInspectorFacade,
      nodeResolve({ preferBuiltins: true, exportConditions: ['node'] }),
      commonjs(),
      json(),
    ],
  },
];
