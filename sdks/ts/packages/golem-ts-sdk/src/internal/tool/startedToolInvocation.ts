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

import type { ByteStreamFailure, ByteStreamItem } from 'golem:tool/host@0.1.0';

export type ToolInputStream = ReadableStream<Uint8Array>;

export interface StartedToolInvocation<Result> {
  readonly stdout: ReadableStream<Uint8Array>;
  readonly result: Promise<Result>;
  cancel(): void;
  collect(): Promise<{ result: Result; stdout: Uint8Array }>;
}

export function startedToolInvocation<Result>(
  stdout: AsyncIterable<ByteStreamItem>,
  result: Promise<Result>,
  cancel: () => void,
): StartedToolInvocation<Result> {
  const stream = readableToolStdout(stdout);
  return {
    stdout: stream,
    result,
    cancel,
    async collect() {
      const [resultOutcome, stdoutOutcome] = await Promise.allSettled([
        result,
        collectReadableStream(stream),
      ]);
      if (resultOutcome.status === 'rejected') {
        throw resultOutcome.reason;
      }
      if (stdoutOutcome.status === 'rejected') {
        throw stdoutOutcome.reason;
      }
      return { result: resultOutcome.value, stdout: stdoutOutcome.value };
    },
  };
}

function readableToolStdout(source: AsyncIterable<ByteStreamItem>): ReadableStream<Uint8Array> {
  const iterator = source[Symbol.asyncIterator]();
  return new ReadableStream({
    async pull(controller) {
      const next = await iterator.next();
      if (next.done) return controller.close();
      if (next.value.tag === 'err') {
        controller.error(new Error(`tool stdout failed: ${streamFailureMessage(next.value.val)}`));
        return;
      }
      if (next.value.val.byteLength === 0) {
        controller.error(new Error('tool stdout produced an empty chunk'));
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
