import { componentize } from '@bytecodealliance/componentize-js';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const jsSource = await readFile('main.js', 'utf8');

const { component } = await componentize(jsSource, {
  witPath: resolve('wit'),
  enableStdout: true,
  preview2Adapter: '../../golem-wit/adapters/tier2/wasi_snapshot_preview1.wasm',
  debug: true
});

await writeFile('out/component.wasm', component);
