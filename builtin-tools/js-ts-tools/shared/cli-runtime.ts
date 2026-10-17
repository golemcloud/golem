import { mkdirSync } from 'node:fs';

class CapturedExit {
  readonly name = 'CapturedExit';
}

function callbackFromWriteArgs(
  encodingOrCallback?: BufferEncoding | ((error?: Error | null) => void),
  callback?: (error?: Error | null) => void,
) {
  return typeof encodingOrCallback === 'function' ? encodingOrCallback : callback;
}

function asError(value: unknown): Error {
  return value instanceof Error ? value : new Error(String(value));
}

function outputBridge(stream: WritableStream<Uint8Array>) {
  const writer = stream.getWriter();
  let pending = Promise.resolve();

  const write = (
    chunk: Uint8Array | string,
    encodingOrCallback?: BufferEncoding | ((error?: Error | null) => void),
    callback?: (error?: Error | null) => void,
  ) => {
    const done = callbackFromWriteArgs(encodingOrCallback, callback);
    const encoding = typeof encodingOrCallback === 'string' ? encodingOrCallback : undefined;
    const bytes = typeof chunk === 'string' ? Buffer.from(chunk, encoding) : Uint8Array.from(chunk);
    const operation = pending.then(() => writer.write(bytes));
    pending = operation.then(
      () => done?.(null),
      (error) => {
        done?.(asError(error));
        throw error;
      },
    );
    // A CLI can keep running before its final drain attaches a rejection handler.
    void pending.catch(() => {});
    return true;
  };

  return {
    write,
    drain: () => pending,
    release: () => writer.releaseLock(),
  };
}

export function validatedCwd(value: string): string {
  if (!value.startsWith('/')) throw new Error('cwd must be an absolute path');
  return value.replace(/\/+$/, '') || '/';
}

export async function runCli(
  options: {
    argv: string[];
    cwd: string;
    environment?: Record<string, string>;
    directories?: string[];
    stopOnExit?: boolean;
    waitForRuntimeIdle?: boolean;
    npmExit?: boolean;
  },
  streams: {
    stdout: WritableStream<Uint8Array>;
    stderr: WritableStream<Uint8Array>;
  },
  execute: (processFacade: NodeJS.Process) => Promise<void> | void,
): Promise<number> {
  const cwd = validatedCwd(options.cwd);
  const environment = options.environment ?? {};
  const stdout = outputBridge(streams.stdout);
  const stderr = outputBridge(streams.stderr);
  const original = {
    argv: process.argv,
    cwd: process.cwd(),
    exit: process.exit,
    exitCode: process.exitCode,
    stdoutWrite: process.stdout.write,
    stderrWrite: process.stderr.write,
    // npm exports its configuration to the environment for lifecycle scripts, so restore the
    // complete environment rather than only the variables set here.
    env: { ...process.env },
  };
  const capturedExit = new CapturedExit();
  let exitRequested = false;
  let executeFailed = false;

  try {
    try {
      process.stdout.write = stdout.write as typeof process.stdout.write;
      process.stderr.write = stderr.write as typeof process.stderr.write;
      process.argv = [...options.argv];
      process.exitCode = 0;
      Object.assign(process.env, environment);
      for (const directory of options.directories ?? []) mkdirSync(directory, { recursive: true });
      process.chdir(cwd);
      process.exit = ((code?: string | number | null) => {
        if (code !== undefined && code !== null) process.exitCode = Number(code);
        if (options.npmExit && !exitRequested) {
          exitRequested = true;
          process.emit('exit', Number(process.exitCode ?? 0));
        }
        if (options.stopOnExit) throw capturedExit;
        return undefined as never;
      }) as typeof process.exit;

      const facade = Object.create(process) as NodeJS.Process;
      Object.defineProperties(facade, {
        argv: { value: process.argv, writable: true, configurable: true },
        env: { value: process.env, configurable: true },
        stdout: { value: process.stdout, configurable: true },
        stderr: { value: process.stderr, configurable: true },
        exit: { value: process.exit, writable: true, configurable: true },
        exitCode: {
          get: () => process.exitCode,
          set: (value: number | undefined) => {
            process.exitCode = value;
          },
          configurable: true,
        },
      });

      await execute(facade);
      if (options.waitForRuntimeIdle) {
        const awaitRuntimeIdle = (
          process as NodeJS.Process & {
            _awaitRuntimeIdle?: () => Promise<void>;
          }
        )._awaitRuntimeIdle;
        if (typeof awaitRuntimeIdle !== 'function') {
          throw new Error('the JavaScript runtime does not provide an idle boundary');
        }
        await awaitRuntimeIdle();
      }
    } catch (error) {
      if (error !== capturedExit) {
        executeFailed = true;
        process.exitCode = Number(process.exitCode || 1);
        process.stderr.write(
          `${error instanceof Error ? (error.stack ?? error.message) : String(error)}\n`,
        );
      }
    }
    // Write callbacks can enqueue more output on either stream, including npm's
    // final exit callback. Settle both queues before restoring the process state.
    let failure: PromiseRejectedResult | undefined;
    for (;;) {
      const out = stdout.drain();
      const err = stderr.drain();
      const results = await Promise.allSettled([out, err]);
      for (const result of results) {
        if (result.status === 'rejected') failure ??= result;
      }
      if (out === stdout.drain() && err === stderr.drain()) break;
    }
    if (failure) throw failure.reason;
    if (options.npmExit && !exitRequested && !executeFailed) {
      throw new Error('npm CLI returned without an exit request');
    }
    let status = Number(process.exitCode ?? 0);
    if (!Number.isFinite(status) || !Number.isInteger(status)) {
      process.stderr.write('Process exit status must be a finite integer\n');
      status = 1;
      await stderr.drain();
    }
    return ((status % 256) + 256) % 256;
  } finally {
    stdout.release();
    stderr.release();
    process.stdout.write = original.stdoutWrite;
    process.stderr.write = original.stderrWrite;
    process.exit = original.exit;
    process.argv = original.argv;
    process.exitCode = original.exitCode;
    for (const key of Object.keys(process.env)) {
      if (!Object.hasOwn(original.env, key)) delete process.env[key];
    }
    Object.assign(process.env, original.env);
    process.chdir(original.cwd);
  }
}
