import assert from 'node:assert/strict';
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

const original = {
  argv: process.argv,
  cwd: process.cwd(),
  env: { ...process.env },
  exit: process.exit,
  stdout: process.stdout.write,
  stderr: process.stderr.write,
};

async function inWorkspace(name: string, run: (workspace: string) => Promise<void>) {
  const workspace = mkdtempSync(path.join(os.tmpdir(), `golem-embedded-${target}-`));
  let stdout = '';
  let stderr = '';
  const capture =
    (append: (chunk: string) => void) =>
    (chunk: unknown, encodingOrCallback?: unknown, callback?: unknown) => {
      append(String(chunk));
      const done = typeof encodingOrCallback === 'function' ? encodingOrCallback : callback;
      if (typeof done === 'function') queueMicrotask(() => done());
      return true;
    };
  process.stdout.write = capture((chunk) => (stdout += chunk)) as typeof process.stdout.write;
  process.stderr.write = capture((chunk) => (stderr += chunk)) as typeof process.stderr.write;
  process.exit = ((code = 0) => {
    process.exitCode = Number(code);
    return undefined as never;
  }) as typeof process.exit;
  process.exitCode = 0;
  process.chdir(workspace);
  try {
    await run(workspace);
    if (process.exitCode) throw new Error(`exited ${process.exitCode}`);
  } catch (error) {
    throw new Error(`${target} ${name}: ${error}\nstdout:\n${stdout}\nstderr:\n${stderr}`);
  } finally {
    process.argv = original.argv;
    process.stdout.write = original.stdout;
    process.stderr.write = original.stderr;
    process.exit = original.exit;
    for (const key of Object.keys(process.env)) {
      if (!Object.hasOwn(original.env, key)) delete process.env[key];
    }
    Object.assign(process.env, original.env);
    process.chdir(original.cwd);
    rmSync(workspace, { recursive: true, force: true });
  }
}

// npm reports completion through process.exit after flushing its output, and writes its final
// diagnostics from process 'exit' listeners.
async function runNpm(workspace: string, args: string[]) {
  const exited = new Promise<void>((resolve) => {
    process.exit = ((code?: number) => {
      if (code !== undefined) process.exitCode = Number(code);
      process.emit('exit', Number(process.exitCode ?? 0));
      resolve();
      return undefined as never;
    }) as typeof process.exit;
  });
  process.argv = ['node', '/toolchain/npm/node_modules/npm/bin/npm-cli.js', ...args];
  process.env.HOME = path.join(workspace, '.home');
  process.env.NPM_CONFIG_CACHE = path.join(workspace, '.cache');
  process.env.NPM_CONFIG_PREFIX = path.join(workspace, '.prefix');
  process.env.NPM_CONFIG_UPDATE_NOTIFIER = 'false';
  const restore = installPrivateReadOnlyFiles('/toolchain/npm/node_modules/npm', npmPrivateFiles);
  try {
    const loaded = evaluateCommonJs(npmBundleSource, '/toolchain/private/npm-cli.cjs') as {
      default: (facade: NodeJS.Process) => Promise<unknown>;
    };
    await loaded.default(process);
    await Promise.race([
      exited,
      new Promise((_, reject) => {
        setTimeout(() => reject(new Error('npm did not request an exit')), 60_000).unref();
      }),
    ]);
  } finally {
    restore();
  }
}

const manifest = () => JSON.parse(readFileSync('package.json', 'utf8'));

if (target === 'npm') {
  await inWorkspace('install', async (workspace) => {
    writeFileSync(
      'package.json',
      `${JSON.stringify({ name: 'embedded-smoke', version: '1.0.0', private: true }, null, 2)}\n`,
    );
    await runNpm(workspace, ['install', '--ignore-scripts', '--no-audit', '--no-fund']);
    if (!existsSync('package-lock.json')) throw new Error('npm did not create package-lock.json');
  });

  await inWorkspace('init -y', async (workspace) => {
    await runNpm(workspace, ['init', '-y']);
    const created = manifest();
    assert.equal(created.name, path.basename(workspace).toLowerCase());
    assert.equal(created.version, '1.0.0');
    assert.equal(created.main, 'index.js');
    assert.equal(created.license, 'ISC');
    assert.deepEqual(created.scripts, { test: 'echo "Error: no test specified" && exit 1' });
  });

  await inWorkspace('init -y with configuration', async (workspace) => {
    await runNpm(workspace, ['init', '-y', '--init-version=0.1.0', '--init-license=MIT']);
    const created = manifest();
    assert.equal(created.version, '0.1.0');
    assert.equal(created.license, 'MIT');
  });

  await inWorkspace('init -y over an existing manifest', async (workspace) => {
    writeFileSync(
      'package.json',
      `${JSON.stringify(
        {
          name: 'existing-package',
          version: '2.3.4',
          description: 'kept',
          scripts: { build: 'tsc' },
          dependencies: { lodash: '^4.17.21' },
        },
        null,
        2,
      )}\n`,
    );
    await runNpm(workspace, ['init', '-y']);
    const updated = manifest();
    assert.equal(updated.name, 'existing-package');
    assert.equal(updated.version, '2.3.4');
    assert.equal(updated.description, 'kept');
    assert.deepEqual(updated.scripts, { build: 'tsc' });
    assert.deepEqual(updated.dependencies, { lodash: '^4.17.21' });
    assert.equal(updated.license, 'ISC');
  });
} else {
  await inWorkspace('project', async () => {
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
  });
}

console.log(`${target} embedded smoke passed`);
