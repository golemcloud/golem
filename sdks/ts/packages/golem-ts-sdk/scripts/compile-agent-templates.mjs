import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { templateMatrix } from './template-matrix.mjs';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const packageDir = resolve(scriptDir, '..');
const firstManifest = join(packageDir, templateMatrix[0].wrapperDirectory, 'Cargo.toml');

function resolveTargetDir() {
  if (process.env.CARGO_TARGET_DIR) return resolve(process.env.CARGO_TARGET_DIR);
  const result = spawnSync(
    'cargo',
    ['metadata', '--no-deps', '--format-version', '1', '--manifest-path', firstManifest],
    { encoding: 'utf8' },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.stderr.write(result.stderr);
    process.exit(result.status ?? 1);
  }
  return JSON.parse(result.stdout).target_directory;
}

const targetDir = resolveTargetDir();
let canonicalLock;

for (const [index, template] of templateMatrix.entries()) {
  const manifest = join(packageDir, template.wrapperDirectory, 'Cargo.toml');
  const lock = join(packageDir, template.wrapperDirectory, 'Cargo.lock');
  if (index > 0) {
    const rootPackage = `name = "${templateMatrix[0].world}"`;
    if (canonicalLock.split(rootPackage).length - 1 !== 1) {
      throw new Error(`Expected exactly one ${rootPackage} entry in the wrapper Cargo.lock`);
    }
    writeFileSync(lock, canonicalLock.replace(rootPackage, `name = "${template.world}"`));
  }

  const locked = existsSync(lock);
  const result = spawnSync(
    'cargo',
    [
      'build',
      ...(locked ? ['--locked'] : []),
      '--target',
      'wasm32-wasip2',
      '--target-dir',
      targetDir,
      '--manifest-path',
      manifest,
      '--release',
      '--no-default-features',
      '--features',
      'full-p3,golem',
    ],
    { stdio: 'inherit' },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
  if (index === 0) canonicalLock = readFileSync(lock, 'utf8');
}
