// Build the component fixtures before measuring their injected and preinitialized
// full-world WASMs. This is a developer harness, not an SDK unit test.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { performance } from 'node:perf_hooks';
import { rollup } from 'rollup';
import nodeResolve from '@rollup/plugin-node-resolve';
import ts from 'typescript';
import { componentPlugin } from './component.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const out = path.join(root, '.component-measurements');
const baseline = process.argv[2];
fs.mkdirSync(out, { recursive: true });
const rows = [];
const run = (command, args) => execFileSync(command, args, { stdio: 'pipe' });

for (const name of ['empty', 'tool-only', 'agent-only', 'middleware-only', 'mixed']) {
  const main = path.join(root, `tests/components/${name}.ts`);
  const config = {
    fileNames: [main],
    options: {
      target: ts.ScriptTarget.ES2022,
      module: ts.ModuleKind.ESNext,
      moduleResolution: ts.ModuleResolutionKind.Bundler,
    },
  };
  for (const mode of baseline ? ['baseline', 'static'] : ['static']) {
    const sdk = '@golemcloud/golem-ts-sdk';
    const bundle = await rollup({
      input: 'virtual:agent-main',
      external: (id) =>
        /^(golem:|wasi:|node:|wasm-rquickjs:)/.test(id) || (mode === 'baseline' && id === sdk),
      onwarn(warning, warn) {
        if (warning.code !== 'CIRCULAR_DEPENDENCY') warn(warning);
      },
      plugins: [
        mode === 'static'
          ? componentPlugin(config, main)
          : {
              name: 'baseline-entry',
              resolveId(id) {
                if (id === 'virtual:agent-main') return '\0baseline-entry';
              },
              load(id) {
                if (id === '\0baseline-entry')
                  return `export default (async () => await import(${JSON.stringify(main)}))();`;
              },
            },
        nodeResolve({ extensions: ['.ts', '.mjs', '.js'] }),
        {
          name: 'typescript',
          transform(code, id) {
            if (id.endsWith('.ts'))
              return {
                code: ts.transpileModule(code, { compilerOptions: config.options }).outputText,
                map: null,
              };
          },
        },
      ],
    });
    const prefix = path.join(out, `${name}-${mode}`);
    await bundle.write({ file: `${prefix}.mjs`, format: 'es', inlineDynamicImports: true });
    await bundle.close();
    run('wasm-rquickjs', [
      'inject-js',
      '--input',
      mode === 'baseline' ? baseline : path.join(root, 'wasm/agent_guest.wasm'),
      '--output',
      `${prefix}.wasm`,
      '--js',
      `${prefix}.mjs`,
    ]);
    const times = [];
    for (let trial = 0; trial < 3; trial++) {
      const start = performance.now();
      run('wasm-rquickjs', [
        'optimize',
        '--input',
        `${prefix}.wasm`,
        '--output',
        `${prefix}.preinitialized.wasm`,
      ]);
      times.push(performance.now() - start);
    }
    run('wasm-tools', ['strip', `${prefix}.wasm`, '-o', `${prefix}.stripped.wasm`]);
    run('wasm-tools', [
      'strip',
      `${prefix}.preinitialized.wasm`,
      '-o',
      `${prefix}.preinitialized.stripped.wasm`,
    ]);
    run('wasm-tools', ['validate', '--features', 'all', `${prefix}.preinitialized.stripped.wasm`]);
    const wit = run('wasm-tools', ['component', 'wit', `${prefix}.wasm`]).toString();
    for (const exported of [
      'golem:agent/guest@2.0.0',
      'golem:tool/guest@0.1.0',
      'golem:tool/tool-middleware-guest@0.1.0',
      'golem:api/save-snapshot@1.5.0',
      'golem:api/load-snapshot@1.5.0',
    ]) {
      if (!wit.includes(`export ${exported}`))
        throw new Error(`Missing mandatory export ${exported}`);
    }
    const bytes = (suffix) => fs.statSync(prefix + suffix).size;
    const row = {
      name,
      mode,
      js: bytes('.mjs'),
      component: bytes('.wasm'),
      stripped: bytes('.stripped.wasm'),
      preinitialized: bytes('.preinitialized.wasm'),
      preinitializedStripped: bytes('.preinitialized.stripped.wasm'),
      preinitializeMedianMs: Math.round(times.sort((a, b) => a - b)[1]),
    };
    rows.push(row);
    console.log(JSON.stringify(row));
  }
}
fs.writeFileSync(path.join(out, 'sizes.json'), JSON.stringify(rows, null, 2));
