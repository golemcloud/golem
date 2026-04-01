import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const packageDir = path.resolve(scriptDir, '..');

const removePath = (targetPath) => {
  fs.rmSync(targetPath, { recursive: true, force: true });
};

removePath(path.join(packageDir, '.metadata'));
removePath(path.join(packageDir, 'agent-template'));
removePath(path.join(packageDir, 'dist'));
removePath(path.join(packageDir, 'node_modules'));
removePath(path.join(packageDir, 'package-lock.json'));

console.log('\nRun `npm install` before building again.\n');
