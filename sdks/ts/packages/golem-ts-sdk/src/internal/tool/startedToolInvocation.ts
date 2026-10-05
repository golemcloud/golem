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
  collect(): Promise<CollectedToolInvocation<Result>>;
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
  mapValue: (value: Input) => Result,
  mapReason: (reason: unknown) => unknown = (reason) => reason,
): Promise<SettledToolResult<Result>> {
  return Promise.resolve(settledResult).then(
    (outcome): SettledToolResult<Result> => {
      if (outcome.status === 'rejected') {
        try {
          return { status: 'rejected', reason: mapReason(outcome.reason) };
        } catch (reason) {
          return { status: 'rejected', reason };
        }
      }
      try {
        return { status: 'fulfilled', value: mapValue(outcome.value) };
      } catch (reason) {
        return { status: 'rejected', reason };
      }
    },
    (reason: unknown): SettledToolResult<Result> => ({ status: 'rejected', reason }),
  );
}

export function resultFromSettledToolResult<Result>(
  settledResult: SettledToolResult<Result> | PromiseLike<SettledToolResult<Result>>,
): Promise<Result> {
  return Promise.resolve(settledResult).then((outcome) => {
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
  return {
    stdout: stdoutStream,
    stderr: stderrStream,
    get result() {
      return resultFromSettledToolResult(settledResult);
    },
    cancel,
    async collect() {
      const [resultOutcome, stdoutOutcome, stderrOutcome] = await Promise.all([
        settledResult,
        settleToolResult(
          stdoutStream === undefined
            ? Promise.resolve(undefined)
            : collectReadableStream(stdoutStream),
        ),
        settleToolResult(
          stderrStream === undefined
            ? Promise.resolve(undefined)
            : collectReadableStream(stderrStream),
        ),
      ]);
      return {
        result: resultOutcome,
        stdout: stdoutOutcome,
        stderr: stderrOutcome,
      };
    },
  };
}

export function deferredStartedToolInvocation<Result>(
  invocation: Promise<StartedToolInvocation<Result>>,
  hasStdout: boolean,
  hasStderr: boolean,
): StartedToolInvocation<Result> {
  let cancelled = false;
  let started: StartedToolInvocation<Result> | undefined;
  void invocation.then(
    (value) => {
      started = value;
      if (cancelled) value.cancel();
    },
    () => {},
  );
  return {
    stdout: hasStdout ? deferredReadableStream(invocation, 'stdout') : undefined,
    stderr: hasStderr ? deferredReadableStream(invocation, 'stderr') : undefined,
    get result() {
      return invocation.then((started) => started.result);
    },
    cancel() {
      if (started) started.cancel();
      else cancelled = true;
    },
    collect() {
      return invocation.then((started) => started.collect());
    },
  };
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
