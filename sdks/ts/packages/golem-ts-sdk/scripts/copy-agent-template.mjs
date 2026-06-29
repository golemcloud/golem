import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const packageDir = path.resolve(scriptDir, '..');
const manifestPath = path.join(packageDir, 'agent-template', 'Cargo.toml');

// Resolve the actual cargo target directory instead of assuming
// `agent-template/target`. This honors `CARGO_TARGET_DIR`, `build.target-dir`
// in cargo config, and any other cargo configuration, matching wherever the
// `compile-agent-template` step actually wrote the artifact.
function resolveTargetDir() {
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

  return path.join(packageDir, 'agent-template', 'target');
}

const targetDir = resolveTargetDir();

const sourcePath = path.join(targetDir, 'wasm32-wasip2', 'release', 'agent_guest.wasm');
const targetPath = path.join(packageDir, 'wasm', 'agent_guest.wasm');

fs.mkdirSync(path.dirname(targetPath), { recursive: true });
fs.copyFileSync(sourcePath, targetPath);
