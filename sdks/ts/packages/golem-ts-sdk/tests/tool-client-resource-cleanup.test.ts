import { expect, it, vi } from 'vitest';
import { z } from 'zod';
import { decodeDeclaredToolError, getExtendedToolDefinition, toolDefinition } from '../src/tool';
import { client } from '../src/toolClient';
import { s } from '../src/schema/markers';
import { compileSchema } from '../src/schema/adapter';
import { schemaGraphToWit } from '../src/internal/schema-model';
import { SchemaValueStream } from 'golem:core/types@2.0.0';
import { mapSettledToolResult } from '../src/internal/tool/startedToolInvocation';

it('starts every cancellation before waiting for a shared producer to close', async () => {
  let release!: () => void;
  const bothCancelled = new Promise<void>((resolve) => {
    release = resolve;
  });
  let cancellations = 0;
  const close = vi.fn(async () => {
    if (++cancellations === 2) release();
    await bothCancelled;
    return { done: true as const, value: undefined };
  });
  const next = vi.fn();
  const endpoints = await Promise.all(
    [0, 1].map(() =>
      SchemaValueStream.wrap({
        [Symbol.asyncIterator]: () => ({ next, return: close }),
      }),
    ),
  );
  const runtime = client(
    toolDefinition('probe').body((body) => body),
    {
      transport: {
        start: () => ({
          settledResult: Promise.resolve({
            status: 'fulfilled',
            value: {
              result: {
                graph: schemaGraphToWit(compileSchema(s.stream(s.u32())).graph),
                value: {
                  root: 999,
                  valueNodes: endpoints.map((val) => ({ tag: 'stream-value' as const, val })),
                },
              },
            },
          }),
          cancel() {},
        }),
      },
    },
  );
  const result = runtime.probe({});
  const rejected = expect(result).rejects.toMatchObject({
    cause: { tag: 'rpc', error: { tag: 'protocol-error' } },
  });
  try {
    await vi.waitFor(() => expect(close).toHaveBeenCalledTimes(2));
    await rejected;
    expect(next).not.toHaveBeenCalled();
  } finally {
    release();
    await rejected;
  }
});

it('reports asynchronous startup cleanup through the started result, not a thrown Promise', async () => {
  const close = vi.fn(async () => ({ done: true as const, value: undefined }));
  const next = vi.fn();
  const raw = await SchemaValueStream.wrap({
    [Symbol.asyncIterator]: () => ({ next, return: close }),
  });
  const payload = {
    graph: schemaGraphToWit(compileSchema(s.stream(s.u32())).graph),
    value: { root: 0, valueNodes: [{ tag: 'stream-value' as const, val: raw }] },
  };
  const definition = toolDefinition('probe').body((body) =>
    body
      .stdout({ required: true })
      .error('failed', { kind: 'runtime', exitCode: 1, payload: z.string() }),
  );
  const runtime = client(definition, {
    transport: {
      start: () => {
        throw {
          tag: 'remote-tool-error',
          val: { tag: 'custom-error', val: { name: 'failed', payload } },
        };
      },
    },
  });
  const started = runtime.probe({});
  expect(started).not.toBeInstanceOf(Promise);
  await expect(started.result).rejects.toMatchObject({
    name: 'ToolCallError',
    cause: { tag: 'rpc', error: { tag: 'protocol-error' } },
  });
  expect(close).toHaveBeenCalledOnce();
  expect(next).not.toHaveBeenCalled();
});

it('awaits settled-result mappings and retains asynchronous mapping failures', async () => {
  const failure = new Error('mapping failed');
  expect(
    await mapSettledToolResult({ status: 'fulfilled', value: 1 }, async (value) => value + 1),
  ).toEqual({ status: 'fulfilled', value: 2 });
  expect(
    await mapSettledToolResult({ status: 'fulfilled', value: 1 }, async () => {
      throw failure;
    }),
  ).toEqual({ status: 'rejected', reason: failure });
  expect(
    await mapSettledToolResult(
      { status: 'rejected', reason: failure },
      (value) => value,
      async () => {
        throw failure;
      },
    ),
  ).toEqual({ status: 'rejected', reason: failure });
});

it.each(['result', 'custom-error', 'raw-result'])(
  'closes rejected %s streams without pulling',
  async (mode) => {
    const next = vi.fn();
    let finish!: () => void;
    const gate = new Promise<void>((resolve) => {
      finish = resolve;
    });
    const close = vi.fn(async () => {
      await gate;
      throw new Error('return failed');
    });
    const dispose = vi.fn(() => {
      throw new Error('dispose failed');
    });
    const raw = await SchemaValueStream.wrap({
      [Symbol.asyncIterator]: () => ({ next, return: close }),
    });
    const schema = z.object({
      stream: s.stream(s.u32()),
      count: s.u32(),
      secret: s.secret(z.string()),
    });
    const definition = toolDefinition('probe').body((body) =>
      body.returns(schema).error('failed', { kind: 'runtime', exitCode: 1, payload: schema }),
    );
    const payload = {
      graph: schemaGraphToWit(compileSchema(schema).graph),
      value: {
        root: mode === 'raw-result' ? 999 : 2,
        valueNodes: [
          { tag: 'stream-value' as const, val: raw },
          { tag: 'bool-value' as const, val: true },
          { tag: 'record-value' as const, val: [0, 1, 3] },
          { tag: 'secret-value' as const, val: { [Symbol.dispose]: dispose } as never },
        ],
      },
    };
    const runtime = client(definition, {
      transport: {
        start: () => ({
          settledResult: Promise.resolve(
            mode !== 'custom-error'
              ? { status: 'fulfilled' as const, value: { result: payload } }
              : {
                  status: 'rejected' as const,
                  reason: {
                    tag: 'remote-tool-error',
                    val: { tag: 'custom-error', val: { name: 'failed', payload } },
                  },
                },
          ),
          cancel() {},
        }),
      },
    });
    let settled = false;
    const result = Promise.resolve(runtime.probe({})).finally(() => {
      settled = true;
    });
    const rejection = expect(result).rejects.toMatchObject({
      cause: {
        tag: 'rpc',
        error: {
          tag: 'protocol-error',
          val: expect.stringMatching(mode === 'raw-result' ? /out of range/ : /conform/),
        },
      },
    });
    await vi.waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(settled).toBe(false);
    expect(dispose).not.toHaveBeenCalled();
    if (mode !== 'raw-result') expect(payload.value.valueNodes[0].val).toBeUndefined();
    finish();
    await rejection;
    expect(close).toHaveBeenCalledOnce();
    expect(next).not.toHaveBeenCalled();
    expect(dispose).toHaveBeenCalledOnce();
  },
);

it.each(['result', 'failed', 'future'])('retains accepted stream ownership: %s', async (mode) => {
  const close = vi.fn(async () => ({ done: true as const, value: undefined }));
  const next = vi.fn();
  const raw = await SchemaValueStream.wrap({
    [Symbol.asyncIterator]: () => ({ next, return: close }),
  });
  const schema = s.stream(s.u32());
  const definition = toolDefinition('probe').body((body) =>
    body.returns(schema).error('failed', { kind: 'runtime', exitCode: 1, payload: schema }),
  );
  const payload = {
    graph: schemaGraphToWit(compileSchema(schema).graph),
    value: { root: 0, valueNodes: [{ tag: 'stream-value' as const, val: raw }] },
  };
  if (mode === 'result') {
    const runtime = client(definition, {
      transport: {
        start: () => ({
          settledResult: Promise.resolve({ status: 'fulfilled', value: { result: payload } }),
          cancel() {},
        }),
      },
    });
    expect(await runtime.probe({})).toBeDefined();
  } else {
    expect(
      await decodeDeclaredToolError(
        getExtendedToolDefinition(definition).root.body!,
        { name: mode, payload },
        'probe',
      ),
    ).toBeDefined();
  }
  expect(close).not.toHaveBeenCalled();
  expect(next).not.toHaveBeenCalled();
});

it('disposes owned results when their wire schema mismatches the local contract', async () => {
  const dispose = vi.fn();
  const schema = z.object({ secret: s.secret(z.string()), count: s.u32() });
  const definition = toolDefinition('probe').body((body) => body.returns(schema));
  const runtime = client(definition, {
    transport: {
      start: () => ({
        settledResult: Promise.resolve({
          status: 'fulfilled',
          value: {
            result: {
              graph: schemaGraphToWit(
                compileSchema(z.object({ secret: s.secret(z.string()), count: z.boolean() })).graph,
              ),
              value: {
                valueNodes: [
                  { tag: 'secret-value', val: { [Symbol.dispose]: dispose } },
                  { tag: 'bool-value', val: true },
                  { tag: 'record-value', val: [0, 1] },
                ],
                root: 2,
              },
            },
          },
        }),
        cancel() {},
      }),
    },
  });
  await expect(runtime.probe({})).rejects.toMatchObject({
    name: 'ToolCallError',
    cause: {
      tag: 'rpc',
      error: { tag: 'protocol-error', val: expect.stringContaining('schema does not match') },
    },
  });
  expect(dispose).toHaveBeenCalledOnce();
});

it.each(['graph', 'value', 'malformed', 'unknown-malformed'])(
  'disposes rejected custom-error payloads: %s',
  async (mode) => {
    const dispose = vi.fn();
    const schema = z.object({ secret: s.secret(z.string()), count: s.u32() });
    const definition = toolDefinition('probe').body((body) =>
      body.error('failed', { kind: 'runtime', exitCode: 1, payload: schema }),
    );
    const payload = {
      graph: schemaGraphToWit(compileSchema(mode === 'graph' ? z.string() : schema).graph),
      value: {
        root: mode.includes('malformed') ? 999 : 2,
        valueNodes: [
          { tag: 'secret-value' as const, val: { [Symbol.dispose]: dispose } as never },
          { tag: 'bool-value' as const, val: true },
          { tag: 'record-value' as const, val: [0, 1] },
        ],
      },
    };
    const runtime = client(definition, {
      transport: {
        start: () => ({
          settledResult: Promise.resolve({
            status: 'rejected',
            reason: {
              tag: 'remote-tool-error',
              val: {
                tag: 'custom-error',
                val: {
                  name: mode === 'unknown-malformed' ? 'future' : 'failed',
                  payload,
                },
              },
            },
          }),
          cancel() {},
        }),
      },
    });
    await expect(runtime.probe({})).rejects.toMatchObject({
      cause: { tag: 'rpc', error: { tag: 'protocol-error' } },
    });
    expect(dispose).toHaveBeenCalledOnce();
    expect(payload.value.valueNodes[0].val).toBeUndefined();
  },
);

it.each(['failed', 'future'])(
  'preserves resources transferred in custom error %s',
  async (name) => {
    const dispose = vi.fn();
    const schema = s.secret(z.string());
    const definition = toolDefinition('probe').body((body) =>
      body.error('failed', { kind: 'runtime', exitCode: 1, payload: schema }),
    );
    const payload = {
      graph: schemaGraphToWit(compileSchema(schema).graph),
      value: {
        root: 0,
        valueNodes: [{ tag: 'secret-value' as const, val: { [Symbol.dispose]: dispose } as never }],
      },
    };
    const result = await decodeDeclaredToolError(
      getExtendedToolDefinition(definition).root.body!,
      { name, payload },
      'probe',
    );
    expect(result.tag).toBe(name === 'failed' ? 'err' : 'unknown-error');
    expect(dispose).not.toHaveBeenCalled();
    expect(payload.value.valueNodes[0].val !== undefined).toBe(name === 'future');
  },
);

it('disposes all unexpected unit-command resources once, including aliased nodes', async () => {
  const dispose = vi.fn(() => {
    throw new Error('drop failed');
  });
  const sibling = vi.fn();
  const raw = { [Symbol.dispose]: dispose } as never;
  const payload = {
    graph: schemaGraphToWit(compileSchema(z.string()).graph),
    value: {
      root: 0,
      valueNodes: [
        { tag: 'secret-value' as const, val: raw },
        { tag: 'secret-value' as const, val: raw },
        { tag: 'quota-token-handle' as const, val: { [Symbol.dispose]: sibling } as never },
      ],
    },
  };
  const runtime = client(
    toolDefinition('probe').body((body) => body),
    {
      transport: {
        start: () => ({
          settledResult: Promise.resolve({ status: 'fulfilled', value: { result: payload } }),
          cancel() {},
        }),
      },
    },
  );
  await expect(runtime.probe({})).rejects.toMatchObject({
    cause: {
      tag: 'rpc',
      error: { tag: 'protocol-error', val: expect.stringContaining('unexpected result') },
    },
  });
  expect(dispose).toHaveBeenCalledOnce();
  expect(sibling).toHaveBeenCalledOnce();
  expect(payload.value.valueNodes.every((node) => node.val === undefined)).toBe(true);
});

it.each([
  { tag: 'u32-value' as const, val: -1 },
  { tag: 'bool-value' as const, val: true },
])('rejects malformed scalar results and disposes their owned sibling: $tag', async (scalar) => {
  const dispose = vi.fn();
  const schema = z.object({ secret: s.secret(z.string()), count: s.u32() });
  const runtime = client(
    toolDefinition('probe').body((body) => body.returns(schema)),
    {
      transport: {
        start: () => ({
          settledResult: Promise.resolve({
            status: 'fulfilled',
            value: {
              result: {
                graph: schemaGraphToWit(compileSchema(schema).graph),
                value: {
                  valueNodes: [
                    { tag: 'secret-value', val: { [Symbol.dispose]: dispose } },
                    scalar,
                    { tag: 'record-value', val: [0, 1] },
                  ],
                  root: 2,
                },
              },
            },
          }),
          cancel() {},
        }),
      },
    },
  );
  await expect(runtime.probe({})).rejects.toMatchObject({
    name: 'ToolCallError',
    cause: { tag: 'rpc', error: { tag: 'protocol-error' } },
  });
  expect(dispose).toHaveBeenCalledOnce();
});
