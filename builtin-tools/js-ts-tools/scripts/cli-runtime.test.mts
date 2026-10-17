import assert from 'node:assert/strict';
import { mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { runCli, validatedCwd } from '../shared/cli-runtime.ts';

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
  let stdoutText = '';
  const stdout = new WritableStream<Uint8Array>({
    async write(chunk) {
      await new Promise<void>((resolve) => setTimeout(resolve, 20));
      stdoutText += Buffer.from(chunk).toString('utf8');
    },
  });
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
      { stdout, stderr: stderr.stream },
      (processFacade) => {
        assert.deepEqual(processFacade.argv, ['node', '--version']);
        assert.equal(processFacade.cwd(), directory);
        assert.equal(processFacade.env.GOLEM_CLI_RUNTIME_TEST, 'isolated');
        processFacade.stdout.write('out', () => {
          callbackCalled = true;
          processFacade.exitCode = 7;
        });
        processFacade.stderr.write(Uint8Array.from(Buffer.from('err')));
      },
    );

    assert.equal(exitCode, 7, stderr.text());
    assert.equal(stdoutText, 'out');
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
  const runtimeProcess = process as NodeJS.Process & {
    _awaitRuntimeIdle?: () => Promise<void>;
  };
  const original = runtimeProcess._awaitRuntimeIdle;
  Object.defineProperty(runtimeProcess, '_awaitRuntimeIdle', {
    value: () => new Promise<void>((resolve) => setTimeout(resolve, 30)),
    configurable: true,
  });

  try {
    const exitCode = await runCli(
      { argv: ['node', '--eval'], cwd: '/', waitForRuntimeIdle: true },
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

async function completesNpmExitAndCleanup() {
  const output: string[] = [];
  let inFlight = 0;
  const stdout = new WritableStream<Uint8Array>({
    async write(bytes) {
      inFlight++;
      await new Promise((resolve) => setTimeout(resolve, 5));
      output.push(Buffer.from(bytes).toString());
      inFlight--;
    },
  });
  const stderr = capture();
  let exits = 0;
  const onExit = (code: number) => {
    exits++;
    assert.equal(code, 7);
    process.stdout.write('cleanup');
  };
  process.once('exit', onExit);
  try {
    const code = await runCli(
      { argv: ['npm'], cwd: '/', npmExit: true },
      { stdout, stderr: stderr.stream },
      (facade) => {
        // npm's ExitHandler schedules this chain after the command resolves.
        facade.stderr.write('', () => {
          facade.stdout.write('command', () => facade.exit(7));
        });
      },
    );
    assert.equal(code, 7);
    assert.equal(exits, 1);
    assert.deepEqual(output, ['command', 'cleanup']);
    assert.equal(inFlight, 0);
    assert.equal(stdout.locked, false);
  } finally {
    process.removeListener('exit', onExit);
  }
}

async function settlesBothStreamsOnFailure() {
  const failure = new Error('output transport failed');
  const stdout = new WritableStream<Uint8Array>({
    write() {
      throw failure;
    },
  });
  let callbackFinished = false;
  let exitFinished = false;
  const stderr = new WritableStream<Uint8Array>({
    async write() {
      await new Promise((resolve) => setTimeout(resolve, 5));
    },
  });
  const onExit = () => {
    exitFinished = true;
  };
  process.once('exit', onExit);
  const before = process.stdout.write;
  try {
    await assert.rejects(
      runCli({ argv: ['npm'], cwd: '/', npmExit: true }, { stdout, stderr }, (facade) => {
        facade.stdout.write('fails', (error) => {
          assert.equal(error, failure);
          facade.stderr.write('late diagnostic', () => {
            callbackFinished = true;
            facade.exit(1);
          });
        });
      }),
      (error) => error === failure,
    );
    assert.equal(callbackFinished, true);
    assert.equal(exitFinished, true);
    assert.equal(stdout.locked, false);
    assert.equal(stderr.locked, false);
    assert.equal(process.stdout.write, before);
  } finally {
    process.removeListener('exit', onExit);
  }
}

async function requiresNpmExitWithoutWaitingForever() {
  await assert.rejects(
    runCli(
      { argv: ['npm'], cwd: '/', npmExit: true },
      { stdout: capture().stream, stderr: capture().stream },
      () => {},
    ),
    /without an exit request/,
  );
  const stderr = capture();
  assert.equal(
    await runCli(
      { argv: ['npm'], cwd: '/', npmExit: true },
      { stdout: capture().stream, stderr: stderr.stream },
      () => {
        throw new Error('failed before npm installed its exit handler');
      },
    ),
    1,
  );
  assert.match(stderr.text(), /failed before npm/);
}

async function normalizesStatuses() {
  for (const [status, expected] of [
    [0, 0],
    [255, 255],
    [-1, 255],
    [-257, 255],
    [256, 0],
    [513, 1],
  ]) {
    for (const useExit of [false, true]) {
      const stdout = capture();
      const stderr = capture();
      const actual = await runCli(
        { argv: ['node'], cwd: '/', stopOnExit: true },
        { stdout: stdout.stream, stderr: stderr.stream },
        (facade) => {
          if (useExit) facade.exit(status);
          else facade.exitCode = status;
        },
      );
      assert.equal(actual, expected);
    }
  }
  const original = process.exitCode;
  for (const status of [0.5, NaN, Infinity, -Infinity]) {
    for (const useExit of [false, true]) {
      const stdout = capture();
      const stderr = capture();
      const actual = await runCli(
        { argv: ['node'], cwd: '/' },
        { stdout: stdout.stream, stderr: stderr.stream },
        (facade) => {
          if (useExit) facade.exit(status);
          else facade.exitCode = status;
        },
      );
      assert.equal(actual, 1);
      assert.match(stderr.text(), /TypeError|RangeError/);
      assert.equal(process.exitCode, original);
    }
  }
  // QuickJS exposes exitCode as a writable property, unlike Node's validating setter.
  const originalProcess = process;
  const permissiveProcess = Object.create(process) as NodeJS.Process;
  Object.defineProperty(permissiveProcess, 'exitCode', {
    value: original,
    writable: true,
    configurable: true,
  });
  globalThis.process = permissiveProcess;
  try {
    for (const status of [0.5, NaN, Infinity, -Infinity]) {
      const stdout = capture();
      const stderr = capture();
      const actual = await runCli(
        { argv: ['node'], cwd: '/' },
        { stdout: stdout.stream, stderr: stderr.stream },
        (facade) => {
          facade.exitCode = status;
        },
      );
      assert.equal(actual, 1);
      assert.match(stderr.text(), /finite integer/);
      assert.equal(process.exitCode, original);
    }
  } finally {
    globalThis.process = originalProcess;
  }
}

await normalizesStatuses();
await isolatesProcessStateAndForwardsOutput();
await capturesExit();
await capturesErrors();
await waitsForRuntimeIdleBeforeRestoringOutput();
await restoresStateAfterSetupFailure();
await completesNpmExitAndCleanup();
await settlesBothStreamsOnFailure();
await requiresNpmExitWithoutWaitingForever();
validatesCwd();
console.log('CLI runtime checks passed');
