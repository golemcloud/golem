import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { templateMatrix } from './template-matrix.mjs';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const packageDir = path.resolve(scriptDir, '..');

const removePath = (targetPath) => {
  fs.rmSync(targetPath, { recursive: true, force: true });
};

removePath(path.join(packageDir, '.metadata'));
removePath(path.join(packageDir, 'dist'));
removePath(path.join(packageDir, 'node_modules'));
removePath(path.join(packageDir, 'package-lock.json'));
for (const template of templateMatrix) {
  removePath(path.join(packageDir, template.wrapperDirectory));
  removePath(path.join(packageDir, 'wasm', template.wasmFile));
}

console.log('\nRun `npm install` before building again.\n');
