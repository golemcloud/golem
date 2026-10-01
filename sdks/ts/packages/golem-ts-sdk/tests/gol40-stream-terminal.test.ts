// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import { expect, it, vi } from 'vitest';
import { AgentStream, client, s, toolDefinition, type ToolClientTransport } from '../src';
import { compiledToolClient } from '../src/toolClient';

it('encodes a native schema stream for a command that also declares stdout', async () => {
  const definition = toolDefinition('stream-with-stdout').body((body) =>
    body.positional('input', s.stream(s.u32())).stdout({ required: true }),
  );
  const start = vi.fn<ToolClientTransport['start']>(() => ({
    stdout: {
      async *[Symbol.asyncIterator]() {
        yield { tag: 'ok' as const, val: Uint8Array.of(7) };
      },
    },
    settledResult: Promise.resolve({
      status: 'fulfilled' as const,
      value: { result: undefined },
    }),
    cancel() {},
  }));
  const generated = client(definition, { transport: { start } });

  const invocation = generated['stream-with-stdout']({ input: AgentStream.from([2, 5, 9]) });
  const reader = invocation.stdout.getReader();
  await expect(reader.read()).resolves.toEqual({ done: false, value: Uint8Array.of(7) });
  expect(start).toHaveBeenCalledOnce();
});

it('encodes a native schema stream for a compiled command that also declares stdout', async () => {
  const elementCodec = {
    write: (value: unknown, writer: { add(node: unknown): number }) =>
      writer.add({ tag: 'u32-value', val: value }),
    read: () => undefined,
  };
  const inputCodec = {
    write: (
      value: unknown,
      writer: { stream(value: AgentStream<number>, codec: unknown): number },
    ) => writer.stream((value as { input: AgentStream<number> }).input, elementCodec),
    read: () => undefined,
  };
  const start = vi.fn<ToolClientTransport['start']>(() => ({
    stdout: {
      async *[Symbol.asyncIterator]() {
        yield { tag: 'ok' as const, val: Uint8Array.of(11) };
      },
    },
    settledResult: Promise.resolve({ status: 'fulfilled' as const, value: {} }),
    cancel() {},
  }));
  const generated = compiledToolClient(
    'compiled-stream-with-stdout',
    [
      {
        path: [],
        aliases: [],
        nested: false,
        input: { codec: inputCodec, graph: { typeNodes: [], defs: [], root: 0 } },
        errors: {},
        stdout: { required: true },
      },
    ],
    { transport: { start } },
  ) as {
    'compiled-stream-with-stdout'(args: { input: AgentStream<number> }): {
      stdout: ReadableStream<Uint8Array>;
    };
  };

  const invocation = generated['compiled-stream-with-stdout']({
    input: AgentStream.from([2, 5, 9]),
  });
  await expect(invocation.stdout.getReader().read()).resolves.toEqual({
    done: false,
    value: Uint8Array.of(11),
  });
  expect(start).toHaveBeenCalledOnce();
});

it('forwards cancellation after asynchronous stream input encoding has completed', async () => {
  const definition = toolDefinition('deferred-stream-cancel').body((body) =>
    body.positional('input', s.stream(s.u32())).stdout({ required: true }),
  );
  const cancel = vi.fn();
  const start = vi.fn<ToolClientTransport['start']>(() => ({
    stdout: {
      async *[Symbol.asyncIterator]() {
        yield { tag: 'ok' as const, val: Uint8Array.of(1) };
      },
    },
    settledResult: new Promise(() => undefined),
    cancel,
  }));
  const invocation = client(definition, { transport: { start } })['deferred-stream-cancel']({
    input: AgentStream.from([1]),
  });

  await vi.waitFor(() => expect(start).toHaveBeenCalledOnce());
  invocation.cancel();

  expect(cancel).toHaveBeenCalledOnce();
});
