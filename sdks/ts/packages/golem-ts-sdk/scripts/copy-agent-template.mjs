import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { templateMatrix } from './template-matrix.mjs';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const packageDir = path.resolve(scriptDir, '..');

// All wrapper crates compile into the first wrapper's resolved target directory
// so their large shared dependency graph is built only once.
function resolveTargetDir(manifestPath, wrapperDirectory) {
  try {
    const output = execFileSync(
      'cargo',
      ['metadata', '--no-deps', '--format-version', '1', '--manifest-path', manifestPath],
      { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 },
    );
    const { target_directory: targetDirectory } = JSON.parse(output);
    if (targetDirectory) {
      return targetDirectory;
    }
  } catch (err) {
    console.warn(
      `Could not resolve cargo target directory via \`cargo metadata\` (${err.message}); ` +
        `falling back to the default location.`,
    );
  }

  if (process.env.CARGO_TARGET_DIR) {
    return path.resolve(process.env.CARGO_TARGET_DIR);
  }

  return path.join(packageDir, wrapperDirectory, 'target');
}

const targetDir = resolveTargetDir(
  path.join(packageDir, templateMatrix[0].wrapperDirectory, 'Cargo.toml'),
  templateMatrix[0].wrapperDirectory,
);

for (const template of templateMatrix) {
  const sourcePath = path.join(targetDir, 'wasm32-wasip2', 'release', template.cargoArtifact);
  const targetPath = path.join(packageDir, 'wasm', template.wasmFile);

  fs.mkdirSync(path.dirname(targetPath), { recursive: true });
  fs.copyFileSync(sourcePath, targetPath);
}
