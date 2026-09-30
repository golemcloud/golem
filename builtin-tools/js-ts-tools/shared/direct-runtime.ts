import { mkdirSync } from 'node:fs';
import {
  invocationEnvironment,
  type ToolExecutionResult,
  validatedCwd,
  validatedOutputLimit,
} from './contracts.js';

type StreamName = 'stdout' | 'stderr';

class OutputCollector {
  private readonly encoder = new TextEncoder();
  private readonly decoder = new TextDecoder();
  private readonly chunks: Record<StreamName, string[]> = { stdout: [], stderr: [] };
  private used = 0;
  overflowed = false;

  constructor(private readonly limit: number) {}

  write(stream: StreamName, chunk: unknown): void {
    const text = chunk instanceof Uint8Array ? this.decoder.decode(chunk) : String(chunk);
    const bytes = this.encoder.encode(text);
    const available = Math.max(0, this.limit - this.used);
    if (bytes.length > available) this.overflowed = true;
    if (available > 0) {
      const accepted = bytes.length <= available ? bytes : bytes.slice(0, available);
      this.chunks[stream].push(this.decoder.decode(accepted));
      this.used += accepted.length;
    }
  }

  value(stream: StreamName): string {
    return this.chunks[stream].join('');
  }
}

class CapturedExit {
  readonly name = 'CapturedExit';
}

function callbackFromWriteArgs(
  encodingOrCallback?: BufferEncoding | ((error?: Error | null) => void),
  callback?: (error?: Error | null) => void,
) {
  return typeof encodingOrCallback === 'function' ? encodingOrCallback : callback;
}

export async function runInDirectRuntime(
  options: {
    argv: string[];
    cwd: string;
    registry: string;
    maxOutputBytes: number;
    version: string;
    stopOnExit?: boolean;
  },
  execute: (processFacade: NodeJS.Process) => Promise<void> | void,
): Promise<ToolExecutionResult> {
  const cwd = validatedCwd(options.cwd);
  const environment = invocationEnvironment(cwd, options.registry);
  const collector = new OutputCollector(validatedOutputLimit(options.maxOutputBytes));
  const original = {
    argv: process.argv,
    cwd: process.cwd(),
    exit: process.exit,
    exitCode: process.exitCode,
    stdoutWrite: process.stdout.write,
    stderrWrite: process.stderr.write,
    env: Object.fromEntries(
      Object.keys(environment).map((key) => [key, process.env[key]]),
    ) as Record<string, string | undefined>,
  };
  const capturedExit = new CapturedExit();
  const captureWrite = (stream: StreamName) =>
    function (
      chunk: Uint8Array | string,
      encodingOrCallback?: BufferEncoding | ((error?: Error | null) => void),
      callback?: (error?: Error | null) => void,
    ) {
      collector.write(stream, chunk);
      callbackFromWriteArgs(encodingOrCallback, callback)?.(null);
      return true;
    };

  process.argv = [...options.argv];
  process.exitCode = 0;
  Object.assign(process.env, environment);
  for (const directory of [
    environment.HOME,
    environment.NPM_CONFIG_CACHE,
    environment.NPM_CONFIG_PREFIX,
  ]) {
    mkdirSync(directory, { recursive: true });
  }
  process.chdir(cwd);
  process.stdout.write = captureWrite('stdout') as typeof process.stdout.write;
  process.stderr.write = captureWrite('stderr') as typeof process.stderr.write;
  process.exit = ((code?: string | number | null) => {
    if (code !== undefined && code !== null) process.exitCode = Number(code);
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

  try {
    try {
      await execute(facade);
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
    } catch (error) {
      if (error !== capturedExit) throw error;
    }
    return {
      exitCode: Number(process.exitCode ?? 0),
      version: options.version,
      overflowed: collector.overflowed,
      stdout: collector.value('stdout'),
      stderr: collector.value('stderr'),
    };
  } catch (error) {
    collector.write(
      'stderr',
      `${error instanceof Error ? (error.stack ?? error.message) : String(error)}\n`,
    );
    return {
      exitCode: Number(process.exitCode || 1),
      version: options.version,
      overflowed: collector.overflowed,
      stdout: collector.value('stdout'),
      stderr: collector.value('stderr'),
    };
  } finally {
    process.stdout.write = original.stdoutWrite;
    process.stderr.write = original.stderrWrite;
    process.exit = original.exit;
    process.argv = original.argv;
    process.exitCode = original.exitCode;
    for (const [key, value] of Object.entries(original.env)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    process.chdir(original.cwd);
  }
}
