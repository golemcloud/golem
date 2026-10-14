import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import {
  npmBundleSource,
  npmPrivateFiles,
} from '../generated/npm-assets.ts';
import {
  tscBundleSource,
  typescriptPrivateFiles,
} from '../generated/typescript-assets.ts';
import { evaluateCommonJs, installPrivateReadOnlyFiles } from '../shared/private-vfs.ts';

const target = process.argv[2];
if (target !== 'npm' && target !== 'tsc') throw new Error('expected npm or tsc');

const workspace = mkdtempSync(path.join(os.tmpdir(), `golem-embedded-${target}-`));
const original = {
  argv: process.argv,
  cwd: process.cwd(),
  exit: process.exit,
  stdout: process.stdout.write,
  stderr: process.stderr.write,
};
let stdout = '';
let stderr = '';
process.stdout.write = ((chunk: unknown) => {
  stdout += String(chunk);
  return true;
}) as typeof process.stdout.write;
process.stderr.write = ((chunk: unknown) => {
  stderr += String(chunk);
  return true;
}) as typeof process.stderr.write;
process.exit = ((code = 0) => {
  process.exitCode = Number(code);
  return undefined as never;
}) as typeof process.exit;
process.chdir(workspace);

try {
  if (target === 'npm') {
    writeFileSync(
      'package.json',
      `${JSON.stringify({ name: 'embedded-smoke', version: '1.0.0', private: true }, null, 2)}\n`,
    );
    process.argv = [
      'node',
      '/toolchain/npm/node_modules/npm/bin/npm-cli.js',
      'install',
      '--ignore-scripts',
      '--no-audit',
      '--no-fund',
    ];
    process.env.HOME = path.join(workspace, '.home');
    process.env.NPM_CONFIG_CACHE = path.join(workspace, '.cache');
    process.env.NPM_CONFIG_PREFIX = path.join(workspace, '.prefix');
    process.env.NPM_CONFIG_UPDATE_NOTIFIER = 'false';
    const restore = installPrivateReadOnlyFiles(
      '/toolchain/npm/node_modules/npm',
      npmPrivateFiles,
    );
    try {
      const loaded = evaluateCommonJs(
        npmBundleSource,
        '/toolchain/private/npm-cli.cjs',
      ) as { default: (facade: NodeJS.Process) => Promise<unknown> };
      await loaded.default(process);
    } finally {
      restore();
    }
    if (!existsSync('package-lock.json')) throw new Error('npm did not create package-lock.json');
  } else {
    writeFileSync('index.ts', 'const answer: number = 42;\nexport { answer };\n');
    writeFileSync(
      'tsconfig.json',
      `${JSON.stringify({ compilerOptions: { noEmit: true, strict: true }, files: ['index.ts'] }, null, 2)}\n`,
    );
    process.argv = [
      'node',
      '/toolchain/typescript/node_modules/typescript/bin/tsc',
      '--project',
      'tsconfig.json',
    ];
    const restore = installPrivateReadOnlyFiles(
      '/toolchain/typescript/node_modules/typescript',
      typescriptPrivateFiles,
    );
    try {
      evaluateCommonJs(tscBundleSource, '/toolchain/private/tsc.cjs');
    } finally {
      restore();
    }
  }
  if (process.exitCode) {
    throw new Error(`${target} exited ${process.exitCode}\nstdout:\n${stdout}\nstderr:\n${stderr}`);
  }
} finally {
  process.argv = original.argv;
  process.stdout.write = original.stdout;
  process.stderr.write = original.stderr;
  process.exit = original.exit;
  process.chdir(original.cwd);
  rmSync(workspace, { recursive: true, force: true });
}

console.log(`${target} embedded smoke passed`);
