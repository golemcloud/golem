import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const packageDir = path.resolve(scriptDir, '..');

const sourcePath = path.join(
  packageDir,
  'agent-template',
  'target',
  'wasm32-wasip1',
  'release',
  'agent_guest.wasm'
);
const targetPath = path.join(packageDir, 'wasm', 'agent_guest.wasm');

fs.mkdirSync(path.dirname(targetPath), { recursive: true });
fs.copyFileSync(sourcePath, targetPath);
