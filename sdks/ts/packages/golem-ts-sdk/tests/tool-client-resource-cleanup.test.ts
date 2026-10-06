import { expect, it, vi } from 'vitest';
import { z } from 'zod';
import { toolDefinition } from '../src/tool';
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
