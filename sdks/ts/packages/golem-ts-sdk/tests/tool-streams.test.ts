// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { describe, expect, it, vi } from 'vitest';
import { startedToolInvocation } from '../src/bridge/tool';

async function* chunks(...values: Uint8Array[]) {
  for (const value of values) yield { tag: 'ok' as const, val: value };
}

async function collectStream(stream: ReadableStream<Uint8Array>): Promise<Uint8Array> {
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

describe('started tool invocations', () => {
  it('exposes stdout before its independently awaitable structured result', async () => {
    let finish!: (value: string) => void;
    const result = new Promise<string>((resolve) => (finish = resolve));
    const invocation = startedToolInvocation(chunks(Uint8Array.of(1, 2)), result, vi.fn());
    const reader = invocation.stdout.getReader();

    await expect(reader.read()).resolves.toEqual({ done: false, value: Uint8Array.of(1, 2) });
    finish('done');
    await expect(invocation.result).resolves.toBe('done');
  });

  it('cancels without consuming the result observer', async () => {
    const cancel = vi.fn();
    const invocation = startedToolInvocation(chunks(), Promise.resolve(undefined), cancel);
    invocation.cancel();
    expect(cancel).toHaveBeenCalledOnce();
    await expect(invocation.result).resolves.toBeUndefined();
  });

  it('collects stdout and result concurrently without deadlocking', async () => {
    const invocation = startedToolInvocation(
      chunks(Uint8Array.of(1), Uint8Array.of(2, 3)),
      Promise.resolve(42),
      vi.fn(),
    );
    await expect(invocation.collect()).resolves.toEqual({
      result: 42,
      stdout: Uint8Array.of(1, 2, 3),
    });
  });

  it('surfaces a terminal attachment failure', async () => {
    async function* failed() {
      yield { tag: 'err' as const, val: { tag: 'cancelled' as const } };
    }
    const invocation = startedToolInvocation(failed(), Promise.resolve(undefined), vi.fn());
    await expect(invocation.stdout.getReader().read()).rejects.toThrow(
      'tool stdout failed: cancelled',
    );
  });

  it('keeps stdout consumable after a structured result failure', async () => {
    let controller: ReadableStreamDefaultController<
      { tag: 'ok'; val: Uint8Array } | { tag: 'err'; val: { tag: 'cancelled' } }
    >;
    const stdout = new ReadableStream({
      start(value) {
        controller = value;
      },
    });
    const failure = new Error('structured result failed');
    const invocation = startedToolInvocation(stdout, Promise.reject(failure), vi.fn());

    await expect(invocation.result).rejects.toBe(failure);
    controller!.enqueue({ tag: 'ok', val: Uint8Array.of(1, 2, 3) });
    controller!.close();

    await expect(collectStream(invocation.stdout)).resolves.toEqual(Uint8Array.of(1, 2, 3));
  });

  it('waits for stdout to terminate before collect reports a structured failure', async () => {
    let closeStdout!: () => void;
    const stdout = new ReadableStream<
      { tag: 'ok'; val: Uint8Array } | { tag: 'err'; val: { tag: 'cancelled' } }
    >({
      start(controller) {
        closeStdout = () => controller.close();
      },
    });
    const failure = new Error('structured result failed');
    const invocation = startedToolInvocation(stdout, Promise.reject(failure), vi.fn());
    const collect = invocation.collect();
    let settled = false;
    void collect.then(
      () => {
        settled = true;
      },
      () => {
        settled = true;
      },
    );

    await Promise.resolve();
    await Promise.resolve();
    expect(settled).toBe(false);

    closeStdout();
    await expect(collect).rejects.toBe(failure);
  });
});
