import assert from 'node:assert/strict';
import fs from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { npmBundleSource, npmPrivateFiles } from '../generated/npm-assets.ts';
import { tscBundleSource, typescriptPrivateFiles } from '../generated/typescript-assets.ts';
import { runCli } from '../shared/cli-runtime.ts';
import { evaluateCommonJs, installPrivateReadOnlyFiles } from '../shared/private-vfs.ts';

function delayedOutput() {
  const chunks: Uint8Array[] = [];
  let pending = 0;
  return {
    stream: new WritableStream<Uint8Array>({
      async write(bytes) {
        pending++;
        await new Promise((resolve) => setTimeout(resolve, 5));
        chunks.push(Uint8Array.from(bytes));
        pending--;
      },
    }),
    text: () => Buffer.concat(chunks).toString(),
    pending: () => pending,
  };
}

const workspace = fs.realpathSync(fs.mkdtempSync(join(tmpdir(), 'golem-cli-completion-')));
try {
  for (const [args, expectedCode, diagnostic] of [
    [['--version'], 0, '10.9.9'],
    [['not-a-real-npm-command'], 1, 'Unknown command'],
  ] as const) {
    const stdout = delayedOutput();
    const stderr = delayedOutput();
    const listeners = ['exit', 'uncaughtException', 'unhandledRejection'] as const;
    const before = listeners.map((event) => process.listenerCount(event));
    const restore = installPrivateReadOnlyFiles('/toolchain/npm/node_modules/npm', npmPrivateFiles);
    try {
      const code = await runCli(
        {
          argv: ['node', '/toolchain/npm/node_modules/npm/bin/npm-cli.js', ...args],
          cwd: workspace,
          npmExit: true,
          environment: {
            HOME: workspace,
            NPM_CONFIG_USERCONFIG: join(workspace, 'user.npmrc'),
            NPM_CONFIG_GLOBALCONFIG: join(workspace, 'global.npmrc'),
            NPM_CONFIG_CACHE: join(workspace, 'cache'),
            NPM_CONFIG_UPDATE_NOTIFIER: 'false',
            NPM_CONFIG_OFFLINE: 'true',
            NO_COLOR: '1',
          },
        },
        { stdout: stdout.stream, stderr: stderr.stream },
        async (facade) => {
          const cli = evaluateCommonJs(npmBundleSource, '/toolchain/private/npm-cli.cjs') as {
            default: (facade: NodeJS.Process) => Promise<void>;
          };
          await cli.default(facade);
        },
      );
      assert.equal(code, expectedCode, stderr.text());
      assert.ok(stdout.text().includes(diagnostic), stdout.text());
      assert.equal(stdout.pending() + stderr.pending(), 0);
      assert.deepEqual(
        listeners.map((event) => process.listenerCount(event)),
        before,
      );
    } finally {
      restore();
    }
  }

  fs.writeFileSync(join(workspace, 'main.ts'), 'export const value: number = 42;\n');
  fs.writeFileSync(
    join(workspace, 'tsconfig.json'),
    JSON.stringify({
      compilerOptions: {
        strict: true,
        skipLibCheck: true,
        types: [],
        outDir: 'out',
      },
      files: ['main.ts'],
    }),
  );
  for (const unreadable of [false, true]) {
    const stdout = delayedOutput();
    const stderr = delayedOutput();
    const originalRead = fs.readFileSync;
    // Fault injection is deterministic even when the test runs as root.
    if (unreadable) {
      fs.readFileSync = ((file: Parameters<typeof fs.readFileSync>[0], ...rest: unknown[]) => {
        if (String(file).endsWith('/tsconfig.json') || file === 'tsconfig.json') {
          throw Object.assign(new Error('controlled config read failure'), {
            code: 'EACCES',
          });
        }
        return Reflect.apply(originalRead, fs, [file, ...rest]);
      }) as typeof fs.readFileSync;
    }
    const restore = installPrivateReadOnlyFiles(
      '/toolchain/typescript/node_modules/typescript',
      typescriptPrivateFiles,
    );
    try {
      const args = unreadable
        ? ['--project', 'tsconfig.json']
        : ['--build', '--generateCpuProfile', 'profile.json'];
      const code = await runCli(
        {
          argv: [
            'node',
            '/toolchain/typescript/node_modules/typescript/bin/tsc',
            ...args,
            '--pretty',
            'false',
          ],
          cwd: workspace,
          stopOnExit: true,
        },
        { stdout: stdout.stream, stderr: stderr.stream },
        () => {
          evaluateCommonJs(tscBundleSource, '/toolchain/private/tsc.cjs');
        },
      );
      assert.equal(code, unreadable ? 1 : 0, stdout.text() + stderr.text());
      assert.equal(stderr.text(), '');
      if (unreadable) assert.match(stdout.text(), /error TS5083/);
      else assert.equal(stdout.text(), '');
      assert.equal(stdout.pending() + stderr.pending(), 0);
    } finally {
      restore();
      fs.readFileSync = originalRead;
    }
  }
} finally {
  fs.rmSync(workspace, { recursive: true, force: true });
}
console.log('Bundled CLI completion checks passed');
