import { expect, it, vi } from 'vitest';
import { z } from 'zod';
import { decodeDeclaredToolError, getExtendedToolDefinition, toolDefinition } from '../src/tool';
import { client } from '../src/toolClient';
import { s } from '../src/schema/markers';
import { compileSchema } from '../src/schema/adapter';
import { schemaGraphToWit } from '../src/internal/schema-model';

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

it.each(['failed', 'future'])('preserves resources transferred in custom error %s', (name) => {
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
  const result = decodeDeclaredToolError(
    getExtendedToolDefinition(definition).root.body!,
    { name, payload },
    'probe',
  );
  expect(result.tag).toBe(name === 'failed' ? 'err' : 'unknown-error');
  expect(dispose).not.toHaveBeenCalled();
  expect(payload.value.valueNodes[0].val !== undefined).toBe(name === 'future');
});

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
