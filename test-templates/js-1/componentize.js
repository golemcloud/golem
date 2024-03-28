import { componentize } from '@bytecodealliance/componentize-js';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const jsSource = await readFile('main.js', 'utf8');

const { component } = await componentize(jsSource, {
  witPath: resolve('wit'),
  enableStdout: true,
  preview2Adapter: '../../golem-worker-executor-base/golem-wit/adapters/tier1/wasi_snapshot_preview1.wasm'
});

await writeFile('out/component.wasm', component);
