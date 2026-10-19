import { describe, expect, it, vi } from 'vitest';
import { z } from 'zod';
import '../src/schema/zod';
import { WasmRpc } from 'golem:agent/host@2.0.0';
import { defineAgentClient } from '../src/client';
import { method } from '../src/method';
import { s } from '../src/schema/markers';
import { compileSchema } from '../src/schema/adapter';
import { schemaValueFromWit, schemaValueToWit } from '../src/internal/schema-model';

describe('invocation-equivalent RPC wire codecs', () => {
  it('matches ordinary records without rerunning transforms and uses fresh arenas', async () => {
    let transformed = 0;
    const schema = z.object({
      bytes: z.array(s.u8()),
      label: z.string().transform((value) => {
        transformed++;
        return `${value}!`;
      }),
      rounded: s.f32(),
    });
    const codec = compileSchema(schema);
    const definition = defineAgentClient({
      name: 'WireRpc',
      id: {},
      methods: {
        echo: method({ input: { value: schema }, returns: schema }),
      },
    });
    const client = definition.client.get({});
    const rpc = vi.mocked(WasmRpc.create).mock.results.at(-1)!.value;
    const arenas = [];
    for (const bytes of [[7, 31, 255], [9]]) {
      const value = { bytes, label: 'ready!', rounded: 1.1, extra: 'ignored' };
      rpc.asyncInvokeAndAwait.mockReturnValue({
        metadata: { agentId: 'wire-rpc', idempotencyKey: 'key' },
        future: {
          get: vi.fn().mockResolvedValue(schemaValueToWit(codec.toValue(value))),
          cancel: vi.fn(),
        },
      });
      expect(await client.echo({ value })).toEqual(codec.fromValue(codec.toValue(value)));
      const tree = rpc.asyncInvokeAndAwait.mock.calls.at(-1)![1];
      const decoded = schemaValueFromWit(tree);
      expect(decoded).toEqual({ tag: 'record', fields: [codec.toValue(value)] });
      arenas.push(tree.valueNodes);
    }
    expect(arenas[0]).not.toBe(arenas[1]);
    expect(transformed).toBe(0);
    await expect(
      client.echo({ value: { bytes: [256], label: 'x', rounded: 1 } }),
    ).rejects.toThrow();
  });

  it('retains whole-arena failure and cancellation on the raw transport', async () => {
    const definition = defineAgentClient({
      name: 'WireRpcCancellation',
      id: {},
      methods: {
        echo: method({ input: { value: z.string() }, returns: z.string() }),
      },
    });
    const client = definition.client.get({});
    const rpc = vi.mocked(WasmRpc.create).mock.results.at(-1)!.value;
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: { agentId: 'wire-rpc', idempotencyKey: 'key' },
      future: {
        get: vi.fn().mockResolvedValue({
          valueNodes: [{ tag: 'string-value', val: 'valid root' }, null as never],
          root: 0,
        }),
        cancel: vi.fn(),
      },
    });
    await expect(client.echo({ value: 'x' })).rejects.toThrow(/invalid schema value/);
    const cancel = vi.fn();
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: { agentId: 'wire-rpc', idempotencyKey: 'key' },
      future: { get: vi.fn().mockReturnValue(new Promise(() => {})), cancel },
    });
    const controller = new AbortController();
    const pending = client.echo({ value: 'x' }, { signal: controller.signal });
    await vi.waitFor(() => expect(rpc.asyncInvokeAndAwait).toHaveBeenCalledTimes(2));
    controller.abort();
    await expect(pending).rejects.toThrow();
    expect(cancel).toHaveBeenCalledOnce();
  });
});
