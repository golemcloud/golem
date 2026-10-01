import assert from 'node:assert/strict';
import { mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { runCli, validatedCwd } from '../shared/cli-runtime.ts';

const runtimeProcess = process as NodeJS.Process & {
  _awaitRuntimeIdle?: () => Promise<void>;
};
Object.defineProperty(runtimeProcess, '_awaitRuntimeIdle', {
  value: async () => undefined,
  configurable: true,
});

function capture() {
  const chunks: Uint8Array[] = [];
  return {
    stream: new WritableStream<Uint8Array>({
      write(chunk) {
        chunks.push(Uint8Array.from(chunk));
      },
    }),
    text: () => Buffer.concat(chunks).toString('utf8'),
  };
}

async function isolatesProcessStateAndForwardsOutput() {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'golem-cli-runtime-')));
  const stdout = capture();
  const stderr = capture();
  const before = {
    argv: process.argv,
    cwd: process.cwd(),
    marker: process.env.GOLEM_CLI_RUNTIME_TEST,
    exitCode: process.exitCode,
  };
  let callbackCalled = false;

  try {
    const exitCode = await runCli(
      {
        argv: ['node', '--version'],
        cwd: directory,
        environment: { GOLEM_CLI_RUNTIME_TEST: 'isolated' },
      },
      { stdout: stdout.stream, stderr: stderr.stream },
      (processFacade) => {
        assert.deepEqual(processFacade.argv, ['node', '--version']);
        assert.equal(processFacade.cwd(), directory);
        assert.equal(processFacade.env.GOLEM_CLI_RUNTIME_TEST, 'isolated');
        processFacade.stdout.write('out', () => {
          callbackCalled = true;
        });
        processFacade.stderr.write(Uint8Array.from(Buffer.from('err')));
        processFacade.exitCode = 7;
      },
    );

    assert.equal(exitCode, 7, stderr.text());
    assert.equal(stdout.text(), 'out');
    assert.equal(stderr.text(), 'err');
    assert.equal(callbackCalled, true);
    assert.equal(process.argv, before.argv);
    assert.equal(process.cwd(), before.cwd);
    assert.equal(process.env.GOLEM_CLI_RUNTIME_TEST, before.marker);
    assert.equal(process.exitCode, before.exitCode);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

async function capturesExit() {
  const stdout = capture();
  const stderr = capture();
  const exitCode = await runCli(
    { argv: ['npm'], cwd: '/', stopOnExit: true },
    { stdout: stdout.stream, stderr: stderr.stream },
    (processFacade) => {
      processFacade.stdout.write('before exit\n');
      processFacade.exit(23);
      processFacade.stdout.write('unreachable');
    },
  );

  assert.equal(exitCode, 23);
  assert.equal(stdout.text(), 'before exit\n');
  assert.equal(stderr.text(), '');
}

async function capturesErrors() {
  const stdout = capture();
  const stderr = capture();
  const exitCode = await runCli(
    { argv: ['tsc'], cwd: '/' },
    { stdout: stdout.stream, stderr: stderr.stream },
    () => {
      throw new Error('synthetic failure');
    },
  );

  assert.equal(exitCode, 1);
  assert.equal(stdout.text(), '');
  assert.match(stderr.text(), /synthetic failure/);
}

async function waitsForRuntimeIdleBeforeRestoringOutput() {
  const stdout = capture();
  const stderr = capture();
  const original = runtimeProcess._awaitRuntimeIdle;
  Object.defineProperty(runtimeProcess, '_awaitRuntimeIdle', {
    value: () => new Promise<void>((resolve) => setTimeout(resolve, 30)),
    configurable: true,
  });

  try {
    const exitCode = await runCli(
      { argv: ['node', '--eval'], cwd: '/' },
      { stdout: stdout.stream, stderr: stderr.stream },
      (processFacade) => {
        setTimeout(() => processFacade.stdout.write('late output'), 20);
      },
    );

    assert.equal(exitCode, 0);
    assert.equal(stdout.text(), 'late output');
    assert.equal(stderr.text(), '');
  } finally {
    if (original === undefined) delete runtimeProcess._awaitRuntimeIdle;
    else {
      Object.defineProperty(runtimeProcess, '_awaitRuntimeIdle', {
        value: original,
        configurable: true,
      });
    }
  }
}

async function restoresStateAfterSetupFailure() {
  const stdout = capture();
  const stderr = capture();
  const beforeCwd = process.cwd();
  const exitCode = await runCli(
    {
      argv: ['node'],
      cwd: join(tmpdir(), `golem-cli-runtime-missing-${process.pid}-${Date.now()}`),
    },
    { stdout: stdout.stream, stderr: stderr.stream },
    () => assert.fail('CLI body must not run after setup failure'),
  );

  assert.equal(exitCode, 1);
  assert.equal(stdout.text(), '');
  assert.match(stderr.text(), /ENOENT/);
  assert.equal(process.cwd(), beforeCwd);
}

function validatesCwd() {
  assert.equal(validatedCwd('/workspace///'), '/workspace');
  assert.equal(validatedCwd('/'), '/');
  assert.throws(() => validatedCwd('relative'), /absolute path/);
}

await isolatesProcessStateAndForwardsOutput();
await capturesExit();
await capturesErrors();
await waitsForRuntimeIdleBeforeRestoringOutput();
await restoresStateAfterSetupFailure();
validatesCwd();
console.log('CLI runtime checks passed');
