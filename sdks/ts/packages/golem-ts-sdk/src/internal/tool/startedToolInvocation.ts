// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import type { ByteStreamFailure, ByteStreamItem } from 'golem:tool/streams@0.1.0';

export type ToolInputStream = ReadableStream<Uint8Array>;

export class ToolStreamError extends Error {
  readonly failure: ByteStreamFailure;

  constructor(failure: ByteStreamFailure) {
    super(`tool stream failed: ${streamFailureMessage(failure)}`);
    this.name = 'ToolStreamError';
    this.failure = failure;
  }
}

export function toolStreamFailureFromError(error: unknown): ByteStreamFailure {
  return error instanceof ToolStreamError
    ? error.failure
    : { tag: 'failed', val: error instanceof Error ? error.message : String(error) };
}

export type SettledToolResult<Result> =
  | { readonly status: 'fulfilled'; readonly value: Result }
  | { readonly status: 'rejected'; readonly reason: unknown };

export interface CollectedToolInvocation<Result> {
  readonly result: SettledToolResult<Result>;
  readonly stdout: SettledToolResult<Uint8Array | undefined>;
  readonly stderr: SettledToolResult<Uint8Array | undefined>;
}

export interface StartedToolInvocation<Result> {
  readonly stdout?: ReadableStream<Uint8Array>;
  readonly stderr?: ReadableStream<Uint8Array>;
  readonly result: Promise<Result>;
  cancel(): void;
  /** Stop result delivery without cancelling the invocation or either output. */
  detachResult(): void;
  collect(): Promise<CollectedToolInvocation<Result>>;
}

/** Ownership follows the SDK mapping seam used by generated and reflected clients. */
export interface ToolResultObserver {
  readonly detached: boolean;
  readonly detachment: Promise<void>;
  delivered(): void;
  discard(): void;
  detach(): void;
}

const resultObservers = new WeakMap<object, ToolResultObserver>();
const invocationResults = new WeakMap<object, Promise<SettledToolResult<unknown>>>();
const decodedToolErrors = new WeakSet<object>();

/** Generated clients throw decoded tool errors as their intentional terminal. */
export function decodedToolError<T extends object>(error: T): T {
  decodedToolErrors.add(error);
  return error;
}

export function ownToolResult<T extends object>(result: T, observer: ToolResultObserver): T {
  resultObservers.set(result, observer);
  return result;
}

function detachedResult(): { status: 'rejected'; reason: Error } {
  return { status: 'rejected', reason: new Error('tool result observer is detached') };
}

export function deferredToolResult<Result>(
  invocation: Promise<{ settledResult: Promise<SettledToolResult<Result>> }>,
): Promise<SettledToolResult<Result>> {
  let detached = false;
  let resolveDetachment!: () => void;
  let owner: ToolResultObserver | undefined;
  void invocation.then(
    (started) => {
      owner = resultObservers.get(started.settledResult);
      if (detached) owner?.detach();
    },
    () => {},
  );
  return ownToolResult(
    invocation
      .then((started) => started.settledResult)
      .catch((reason): SettledToolResult<Result> => ({ status: 'rejected', reason })),
    {
      get detached() {
        return detached;
      },
      detachment: new Promise<void>((resolve) => {
        resolveDetachment = resolve;
      }),
      delivered() {
        owner?.delivered();
      },
      discard() {
        owner?.discard();
      },
      detach() {
        detached = true;
        resolveDetachment();
        owner?.detach();
      },
    },
  );
}

export function settleToolResult<Result>(
  result: PromiseLike<Result>,
): Promise<SettledToolResult<Result>> {
  return Promise.resolve(result).then(
    (value): SettledToolResult<Result> => ({ status: 'fulfilled', value }),
    (reason: unknown): SettledToolResult<Result> => ({ status: 'rejected', reason }),
  );
}

export function mapSettledToolResult<Input, Result>(
  settledResult: SettledToolResult<Input> | PromiseLike<SettledToolResult<Input>>,
  mapValue: (value: Input) => Result | PromiseLike<Result>,
  mapReason: (reason: unknown) => unknown = (reason) => reason,
): Promise<SettledToolResult<Result>> {
  const observer = resultObservers.get(settledResult);
  const mapped = Promise.resolve(settledResult).then(
    async (outcome): Promise<SettledToolResult<Result>> => {
      if (observer?.detached) return detachedResult();
      if (outcome.status === 'rejected') {
        try {
          return { status: 'rejected', reason: await mapReason(outcome.reason) };
        } catch (reason) {
          const forwardedRpc =
            typeof reason === 'object' &&
            reason !== null &&
            'tag' in reason &&
            reason.tag === 'rpc' &&
            'error' in reason &&
            reason.error === outcome.reason;
          if (
            reason !== outcome.reason &&
            !forwardedRpc &&
            !(typeof reason === 'object' && reason !== null && decodedToolErrors.has(reason))
          ) {
            observer?.discard();
          }
          return { status: 'rejected', reason };
        }
      }
      try {
        return { status: 'fulfilled', value: await mapValue(outcome.value) };
      } catch (reason) {
        observer?.discard();
        return { status: 'rejected', reason };
      }
    },
    (reason: unknown): SettledToolResult<Result> => ({ status: 'rejected', reason }),
  );
  return observer ? ownToolResult(mapped, observer) : mapped;
}

export function resultFromSettledToolResult<Result>(
  settledResult: SettledToolResult<Result> | PromiseLike<SettledToolResult<Result>>,
): Promise<Result> {
  const observer = resultObservers.get(settledResult);
  const terminal = observer
    ? Promise.race([settledResult, observer.detachment.then(detachedResult)])
    : Promise.resolve(settledResult);
  return terminal.then((outcome) => {
    if (observer?.detached) throw detachedResult().reason;
    observer?.delivered();
    if (outcome.status === 'rejected') throw outcome.reason;
    return outcome.value;
  });
}

export function startedToolInvocation<Result>(
  stdout: AsyncIterable<ByteStreamItem> | undefined,
  stderr: AsyncIterable<ByteStreamItem> | undefined,
  settledResult: Promise<SettledToolResult<Result>>,
  cancel: () => void,
): StartedToolInvocation<Result> {
  const stdoutStream = stdout && readableToolOutput(stdout, 'stdout');
  const stderrStream = stderr && readableToolOutput(stderr, 'stderr');
  return startedReadableToolInvocation(stdoutStream, stderrStream, settledResult, cancel);
}

/** Map inside the observer lifetime, before public result delivery. */
export function mapStartedToolInvocation<Input, Result>(
  invocation: StartedToolInvocation<Input>,
  mapValue: (value: Input) => Result | PromiseLike<Result>,
): StartedToolInvocation<Result> {
  const settled = invocationResults.get(invocation)! as Promise<SettledToolResult<Input>>;
  return startedReadableToolInvocation(
    invocation.stdout,
    invocation.stderr,
    mapSettledToolResult(settled, mapValue),
    invocation.cancel,
  );
}

/** Await-only calls transfer ownership only after output failures are selected. */
export async function awaitToolInvocationResult<Result>(
  invocation: StartedToolInvocation<Result>,
): Promise<Result> {
  const settled = invocationResults.get(invocation)! as Promise<SettledToolResult<Result>>;
  const observer = resultObservers.get(settled);
  const collected = await collectToolInvocation(invocation.stdout, invocation.stderr, settled);
  if (collected.result.status === 'fulfilled') {
    if (collected.stdout.status === 'rejected' || collected.stderr.status === 'rejected') {
      observer?.discard();
    } else {
      observer?.delivered();
    }
  } else {
    observer?.delivered();
  }
  const result = await resultFromSettledToolResult(collected.result);
  await resultFromSettledToolResult(collected.stdout);
  await resultFromSettledToolResult(collected.stderr);
  return result;
}

async function collectToolInvocation<Result>(
  stdout: ReadableStream<Uint8Array> | undefined,
  stderr: ReadableStream<Uint8Array> | undefined,
  settledResult: Promise<SettledToolResult<Result>>,
): Promise<CollectedToolInvocation<Result>> {
  const [result, stdoutOutcome, stderrOutcome] = await Promise.all([
    settledResult,
    settleToolResult(
      stdout === undefined ? Promise.resolve(undefined) : collectReadableStream(stdout),
    ),
    settleToolResult(
      stderr === undefined ? Promise.resolve(undefined) : collectReadableStream(stderr),
    ),
  ]);
  return { result, stdout: stdoutOutcome, stderr: stderrOutcome };
}

function startedReadableToolInvocation<Result>(
  stdoutStream: ReadableStream<Uint8Array> | undefined,
  stderrStream: ReadableStream<Uint8Array> | undefined,
  settledResult: Promise<SettledToolResult<Result>>,
  cancel: () => void,
): StartedToolInvocation<Result> {
  const observer = resultObservers.get(settledResult);
  let detached = false;
  let resolveDetachment!: () => void;
  const detachment = new Promise<SettledToolResult<Result>>((resolve) => {
    resolveDetachment = () => resolve(detachedResult());
  });
  const pending = Promise.race([
    settledResult,
    detachment,
    ...(observer ? [observer.detachment.then(detachedResult)] : []),
  ]);
  const invocation: StartedToolInvocation<Result> = {
    stdout: stdoutStream,
    stderr: stderrStream,
    get result() {
      if (detached) return Promise.reject(detachedResult().reason);
      return pending.then((outcome) => {
        if (detached || observer?.detached) throw detachedResult().reason;
        observer?.delivered();
        if (outcome.status === 'rejected') throw outcome.reason;
        return outcome.value;
      });
    },
    cancel,
    detachResult() {
      detached = true;
      resolveDetachment();
      observer?.detach();
    },
    async collect() {
      const collected = await collectToolInvocation(stdoutStream, stderrStream, pending);
      if (!detached && !observer?.detached) observer?.delivered();
      return {
        ...collected,
        result: detached || observer?.detached ? detachedResult() : collected.result,
      };
    },
  };
  invocationResults.set(invocation, settledResult);
  return invocation;
}

export function deferredStartedToolInvocation<Result>(
  invocation: Promise<StartedToolInvocation<Result>>,
  hasStdout: boolean,
  hasStderr: boolean,
): StartedToolInvocation<Result> {
  let cancelled = false;
  let started: StartedToolInvocation<Result> | undefined;
  const admitted = invocation.then((value) => {
    started = value;
    if (cancelled) value.cancel();
    return { settledResult: invocationResults.get(value)! as Promise<SettledToolResult<Result>> };
  });
  return startedReadableToolInvocation(
    hasStdout ? deferredReadableStream(invocation, 'stdout') : undefined,
    hasStderr ? deferredReadableStream(invocation, 'stderr') : undefined,
    deferredToolResult(admitted),
    () => {
      if (started) started.cancel();
      else cancelled = true;
    },
  );
}

function deferredReadableStream<Result>(
  invocation: Promise<StartedToolInvocation<Result>>,
  channel: 'stdout' | 'stderr',
): ReadableStream<Uint8Array> {
  let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
  const getReader = async () => {
    if (reader) return reader;
    const stream = (await invocation)[channel];
    if (!stream) throw new TypeError(`required ${channel} stream is missing`);
    return (reader = stream.getReader());
  };
  return new ReadableStream({
    async pull(controller) {
      const next = await (await getReader()).read();
      if (next.done) controller.close();
      else controller.enqueue(next.value);
    },
    async cancel(reason) {
      await (await getReader()).cancel(reason);
    },
  });
}

function readableToolOutput(
  source: AsyncIterable<ByteStreamItem>,
  channel: 'stdout' | 'stderr',
): ReadableStream<Uint8Array> {
  const iterator = source[Symbol.asyncIterator]();
  return new ReadableStream({
    async pull(controller) {
      const next = await iterator.next();
      if (next.done) return controller.close();
      if (next.value.tag === 'err') {
        try {
          await iterator.return?.();
        } catch {
          // Preserve the attachment failure.
        }
        controller.error(new ToolStreamError(next.value.val));
        return;
      }
      if (next.value.val.byteLength === 0) {
        try {
          await iterator.return?.();
        } catch {
          // Preserve the empty-chunk failure.
        }
        controller.error(new Error(`tool ${channel} produced an empty chunk`));
        return;
      }
      controller.enqueue(next.value.val);
    },
    cancel: () => iterator.return?.().then(() => undefined),
  });
}

async function collectReadableStream(stream: ReadableStream<Uint8Array>): Promise<Uint8Array> {
  const chunks: Uint8Array[] = [];
  let length = 0;
  for await (const chunk of stream) {
    chunks.push(chunk);
    length += chunk.byteLength;
  }
  const result = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return result;
}

function streamFailureMessage(failure: ByteStreamFailure): string {
  return failure.tag === 'failed' ? failure.val : failure.tag;
}
