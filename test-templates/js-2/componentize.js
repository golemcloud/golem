import { componentize } from '@bytecodealliance/componentize-js';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const jsSource = await readFile('main.js', 'utf8');

const { component } = await componentize(jsSource, {
  witPath: resolve('wit'),
  enableStdout: true,
  debug: true
});

await writeFile('out/component.wasm', component);
