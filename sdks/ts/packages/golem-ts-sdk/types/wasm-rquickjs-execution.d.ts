declare module 'wasm-rquickjs:execution' {
  import type { Readable } from 'node:stream';

  export type ExecutionLanguage = 'javascript' | 'typescript';
  export type ExecutionOverflowPolicy = 'terminate' | 'truncate';

  export type ExecutionSharedOptions = {
    cwd?: string;
    argv?: string[];
    env?: Record<string, string>;
    timeoutMs?: number;
    maxBytes?: number;
    overflow?: ExecutionOverflowPolicy;
  };

  export type ExecutionEntryOptions = ExecutionSharedOptions & {
    entry: string;
    source?: never;
    language?: never;
  };

  export type ExecutionSourceOptions = ExecutionSharedOptions & {
    source: string;
    entry?: never;
    language?: ExecutionLanguage;
  };

  export type ExecutionOptions = ExecutionEntryOptions | ExecutionSourceOptions;

  export type ExecutionStructuredResult<T = unknown> = {
    value: T;
    overflowed: boolean;
  };

  export type ExecutionResult<T = unknown> = ExecutionStructuredResult<T> & {
    stdout: string;
    stderr: string;
  };

  export type ExecutionJob<T = unknown> = {
    stdout: Readable;
    stderr: Readable;
    result: Promise<ExecutionStructuredResult<T>>;
    cancel(): void;
  };

  export function startJavaScript<T = unknown>(options: ExecutionOptions): ExecutionJob<T>;
  export function runJavaScript<T = unknown>(options: ExecutionOptions): Promise<ExecutionResult<T>>;

  const execution: {
    startJavaScript: typeof startJavaScript;
    runJavaScript: typeof runJavaScript;
  };
  export default execution;
}
