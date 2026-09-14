import { spawnSync } from 'node:child_process';
import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
} from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { templateMatrix } from './template-matrix.mjs';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const packageDir = resolve(scriptDir, '..');
const sourceWit = resolve(packageDir, '../../wit');
const typesDir = join(packageDir, 'types');
const temporaryDir = join(packageDir, '.generated-types');
const mergedTypesDir = join(temporaryDir, 'merged');
const preservedDeclarations = ['node-sqlite-extensions.d.ts'];

function filesBelow(root) {
  if (!existsSync(root)) return [];
  return readdirSync(root).flatMap((entry) => {
    const path = join(root, entry);
    return statSync(path).isDirectory() ? filesBelow(path) : [path];
  });
}

function generate(world, output) {
  const result = spawnSync(
    'wasm-rquickjs',
    [
      'generate-dts',
      '--wit',
      sourceWit,
      '--output',
      output,
      '--world',
      world,
      '--target',
      'wasi-p3',
    ],
    { stdio: 'inherit' },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

function mergeSharedDeclarations(candidateDir, targetDir) {
  const candidateFiles = filesBelow(candidateDir).filter(
    (path) => relative(candidateDir, path) !== 'exports.d.ts',
  );
  for (const path of candidateFiles) {
    const relativePath = relative(candidateDir, path);
    const target = join(targetDir, relativePath);
    if (existsSync(target)) {
      if (readFileSync(path, 'utf8') !== readFileSync(target, 'utf8')) {
        throw new Error(
          `Generated WIT dependency declaration differs between worlds: ${relativePath}`,
        );
      }
    } else {
      mkdirSync(dirname(target), { recursive: true });
      cpSync(path, target);
    }
  }
}

rmSync(temporaryDir, { recursive: true, force: true });

try {
  for (const [index, template] of templateMatrix.entries()) {
    const output = index === 0 ? mergedTypesDir : join(temporaryDir, template.role);
    generate(template.world, output);
    if (index === 0) continue;

    mergeSharedDeclarations(output, mergedTypesDir);
    cpSync(join(output, 'exports.d.ts'), join(mergedTypesDir, template.declarationFile));
  }
  for (const declaration of preservedDeclarations) {
    const source = join(typesDir, declaration);
    if (existsSync(source)) cpSync(source, join(mergedTypesDir, declaration));
  }

  rmSync(typesDir, { recursive: true, force: true });
  renameSync(mergedTypesDir, typesDir);
} finally {
  rmSync(temporaryDir, { recursive: true, force: true });
}
