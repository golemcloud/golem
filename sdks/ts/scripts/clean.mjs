import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const rootDir = path.resolve(scriptDir, '..');

const removePath = (targetPath) => {
  fs.rmSync(targetPath, { recursive: true, force: true });
};

removePath(path.join(rootDir, 'dist'));
removePath(path.join(rootDir, 'node_modules'));

const packagesDir = path.join(rootDir, 'packages');
if (fs.existsSync(packagesDir)) {
  for (const entry of fs.readdirSync(packagesDir, { withFileTypes: true })) {
    if (!entry.isDirectory()) {
      continue;
    }

    const packageDir = path.join(packagesDir, entry.name);
    removePath(path.join(packageDir, 'dist'));
    removePath(path.join(packageDir, 'node_modules'));
    removePath(path.join(packageDir, '.metadata'));
  }
}

removePath(path.join(packagesDir, 'golem-ts-sdk', 'agent-template'));
removePath(path.join(packagesDir, 'golem-ts-sdk', 'wasm', 'agent_guest.wasm'));
