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

import type {
  InvocationResult,
  ToolError,
  TypedSchemaValue,
  UnderlyingTool,
} from 'golem:tool/common@0.1.0';
import { describe, expect, it, vi } from 'vitest';
import { z } from 'zod/v4';
import {
  createUnderlyingToolClient,
  decodeUnderlyingToolError,
  encodeToolInvokeError,
  ToolUnderlyingMisuseError,
  withInvocationScopedUnderlying,
} from '../src/internal/tool/middlewareRuntime';
import { t, typedSchemaValueToWit, v } from '../src/internal/schema-model';
import { s } from '../src/schema/markers';
import { compileSchema } from '../src/schema/adapter';
import { ToolInvokeError, toolDefinition, type UniversalToolUnderlying } from '../src/tool';

type RawUnderlyingTool = Pick<UnderlyingTool, 'invoke'>;

const unitValue = typedSchemaValueToWit({
  graph: { defs: new Map(), root: t.tuple([]) },
  value: v.tuple([]),
});

function wireValue(schema: Parameters<typeof compileSchema>[0], value: unknown): TypedSchemaValue {
  const codec = compileSchema(schema);
  return typedSchemaValueToWit({ graph: codec.graph, value: codec.toValue(value) });
}

function rejectionOf(promise: Promise<unknown>): Promise<unknown> {
  return promise.then(
    () => Symbol('resolved'),
    (error) => error,
  );
}

function controllableStream(...values: number[]): {
  stream: AsyncIterable<number>;
  close: ReturnType<typeof vi.fn>;
} {
  let index = 0;
  const close = vi.fn(async () => ({ done: true, value: undefined }) as IteratorResult<number>);
  const iterator: AsyncIterator<number> = {
    next: async () =>
      index < values.length
        ? { done: false, value: values[index++] }
        : { done: true, value: undefined },
    return: close,
  };
  return { stream: { [Symbol.asyncIterator]: () => iterator }, close };
}

function mismatchedCapabilityValue(): {
  value: TypedSchemaValue;
  quotaNode: { tag: 'quota-token-handle'; val: unknown };
  permissionNode: { tag: 'permission-card-handle'; val: unknown };
} {
  const quotaNode = { tag: 'quota-token-handle' as const, val: { id: 'quota' } };
  const permissionNode = {
    tag: 'permission-card-handle' as const,
    val: { id: 'permission' },
  };
  return {
    value: {
      graph: wireValue(z.boolean(), true).graph,
      value: {
        valueNodes: [{ tag: 'tuple-value', val: [1, 2] }, quotaNode, permissionNode],
        root: 0,
      },
    } as TypedSchemaValue,
    quotaNode,
    permissionNode,
  };
}

describe('tool middleware runtime foundation', () => {
  it('round-trips every protocol error exactly and keeps stable messages', () => {
    const cases: ReadonlyArray<readonly [ToolError, string]> = [
      [{ tag: 'invalid-tool-name', val: 'backend' }, 'invalid tool name `backend`'],
      [
        { tag: 'invalid-command-path', val: ['nested', 'run'] },
        'invalid command path `nested run`',
      ],
      [{ tag: 'invalid-input', val: 'bad input' }, 'invalid input: bad input'],
      [{ tag: 'constraint-violation', val: 'policy' }, 'constraint violation: policy'],
      [{ tag: 'invalid-result', val: 'bad result' }, 'invalid result: bad result'],
    ];

    for (const [wire, message] of cases) {
      const decoded = decodeUnderlyingToolError(wire, () => {
        throw new Error('custom decoder must not run');
      });
      expect(decoded).toBeInstanceOf(ToolInvokeError);
      expect(decoded).toMatchObject({ cause: wire, message });
      expect(encodeToolInvokeError(decoded, () => unitValue)).toBe(wire);
      expect((decoded as ToolInvokeError<never>).mapTool(() => 'mapped')).toBe(decoded);
    }
  });

  it('decodes, maps, and re-encodes only the custom error arm', () => {
    const payload = wireValue(z.string(), 'backend failed');
    const decoded = decodeUnderlyingToolError(
      { tag: 'custom-error', val: payload } satisfies ToolError,
      (wire) => {
        expect(wire).toBe(payload);
        return { reason: 'backend failed' };
      },
    ) as ToolInvokeError<{ reason: string }>;
    const mapped = decoded.mapTool(({ reason }) => `presented: ${reason}`);

    expect(mapped.cause).toEqual({ tag: 'tool', error: 'presented: backend failed' });
    expect(encodeToolInvokeError(mapped, () => payload)).toEqual({
      tag: 'custom-error',
      val: payload,
    });

    const failedDecode = decodeUnderlyingToolError(
      { tag: 'custom-error', val: payload } satisfies ToolError,
      () => {
        throw new Error('wrong custom payload');
      },
    );
    expect(failedDecode).toMatchObject({
      cause: { tag: 'invalid-result', val: 'wrong custom payload' },
    });
  });

  it('rejects and drains a malformed encoded custom payload', () => {
    const { value, quotaNode, permissionNode } = mismatchedCapabilityValue();

    expect(encodeToolInvokeError(ToolInvokeError.tool('failure'), () => value)).toMatchObject({
      tag: 'invalid-result',
    });
    expect(quotaNode.val).toBeUndefined();
    expect(permissionNode.val).toBeUndefined();
  });

  it('maps a structurally malformed underlying custom payload to invalid-result', async () => {
    const malformedPayload = {
      graph: { typeNodes: [], defs: [], root: 0 },
      value: { valueNodes: [], root: 0 },
    } as TypedSchemaValue;
    const raw = {
      invoke: vi.fn(async () => {
        throw { tag: 'custom-error', val: malformedPayload } satisfies ToolError;
      }),
    } as RawUnderlyingTool;

    await withInvocationScopedUnderlying(raw, undefined, async (underlying) => {
      await expect(underlying.invoke([], unitValue, undefined)).rejects.toMatchObject({
        cause: { tag: 'invalid-result' },
      });
      return {};
    });
  });

  it.each([null, 42, []])(
    'maps malformed underlying invocation result container %j to invalid-result',
    async (result) => {
      const raw = { invoke: vi.fn(async () => result as never) } as RawUnderlyingTool;

      await withInvocationScopedUnderlying(raw, undefined, async (underlying) => {
        await expect(underlying.invoke([], unitValue, undefined)).rejects.toMatchObject({
          cause: { tag: 'invalid-result', val: 'tool invocation result must be an object' },
        });
        return {};
      });
    },
  );

  it.each([null, undefined])(
    'maps a custom error with payload %s to invalid-result',
    async (payload) => {
      const raw = {
        invoke: vi.fn(async () => {
          throw { tag: 'custom-error', val: payload } as ToolError;
        }),
      } as RawUnderlyingTool;

      await withInvocationScopedUnderlying(raw, undefined, async (underlying) => {
        await expect(underlying.invoke([], unitValue, undefined)).rejects.toMatchObject({
          cause: { tag: 'invalid-result' },
        });
        return {};
      });
    },
  );

  it('drains owned handles from a malformed underlying custom payload', async () => {
    const rawQuota = { id: 'quota' } as never;
    const quotaNode = { tag: 'quota-token-handle' as const, val: rawQuota };
    const malformedPayload = {
      graph: { typeNodes: [], defs: [], root: 0 },
      value: { valueNodes: [quotaNode], root: 0 },
    } as TypedSchemaValue;
    const raw = {
      invoke: vi.fn(async () => {
        throw { tag: 'custom-error', val: malformedPayload } satisfies ToolError;
      }),
    } as RawUnderlyingTool;

    await withInvocationScopedUnderlying(raw, undefined, async (underlying) => {
      await expect(underlying.invoke([], unitValue, undefined)).rejects.toMatchObject({
        cause: { tag: 'invalid-result' },
      });
      return {};
    });

    expect(quotaNode.val).toBeUndefined();
  });

  it('decodes typed underlying custom errors through the expected command codec', async () => {
    const definition = toolDefinition('expected-error').body((body) =>
      body
        .returns(z.void())
        .error('backend-failed', { kind: 'runtime', exitCode: 1, payload: z.string() }),
    );
    const raw = {
      invoke: vi.fn(async () => {
        throw {
          tag: 'custom-error',
          val: wireValue(z.string(), 'unavailable'),
        } satisfies ToolError;
      }),
    } as RawUnderlyingTool;

    await withInvocationScopedUnderlying(raw, undefined, async (underlying) => {
      const failure = (await rejectionOf(
        createUnderlyingToolClient(definition, underlying)['expected-error']({}),
      )) as ToolInvokeError<unknown>;

      expect(failure).toBeInstanceOf(ToolInvokeError);
      expect(failure.cause).toEqual({
        tag: 'tool',
        error: {
          tag: 'err',
          name: 'backend-failed',
          hasPayload: true,
          payload: 'unavailable',
        },
      });
      return {};
    });
  });

  it('does not reinterpret unexpected JavaScript exceptions', () => {
    const unexpected = new Error('implementation bug');
    expect(decodeUnderlyingToolError(unexpected, (payload) => payload)).toBe(unexpected);
    expect(() => encodeToolInvokeError(unexpected, (payload) => payload as never)).toThrow(
      unexpected,
    );
  });

  it('maps a malformed underlying typed result to invalid-result', async () => {
    const malformedResult = {
      graph: { typeNodes: [], defs: [], root: 0 },
      value: { valueNodes: [], root: 0 },
    } as TypedSchemaValue;
    const raw = { invoke: vi.fn(async () => ({ result: malformedResult })) } as RawUnderlyingTool;

    await withInvocationScopedUnderlying(raw, undefined, async (underlying) => {
      await expect(underlying.invoke([], unitValue, undefined)).rejects.toMatchObject({
        cause: { tag: 'invalid-result' },
      });
      return {};
    });
  });

  it('rejects an underlying typed result whose value contradicts its own graph', async () => {
    const boolean = wireValue(z.boolean(), true);
    const string = wireValue(z.string(), 'not a boolean');
    const malformedResult = { graph: boolean.graph, value: string.value };
    const raw = { invoke: vi.fn(async () => ({ result: malformedResult })) } as RawUnderlyingTool;

    await withInvocationScopedUnderlying(raw, undefined, async (underlying) => {
      await expect(underlying.invoke([], unitValue, undefined)).rejects.toMatchObject({
        cause: { tag: 'invalid-result' },
      });
      return {};
    });
  });

  it('drains capabilities and closes stdout when the final result carrier is malformed', async () => {
    const { value, quotaNode, permissionNode } = mismatchedCapabilityValue();
    const stdout = controllableStream(1, 2, 3);
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;

    await expect(
      withInvocationScopedUnderlying(raw, undefined, () => ({
        result: value,
        stdout: stdout.stream,
      })),
    ).rejects.toMatchObject({ cause: { tag: 'invalid-result' } });

    expect(quotaNode.val).toBeUndefined();
    expect(permissionNode.val).toBeUndefined();
    expect(stdout.close).toHaveBeenCalledOnce();
  });

  it.each([0, 1, 3])('allows %i sequential underlying calls', async (callCount) => {
    const invoke = vi.fn(async (_path, input): Promise<InvocationResult> => ({ result: input }));
    const raw = { invoke } as RawUnderlyingTool;

    const result = await withInvocationScopedUnderlying(raw, undefined, async (underlying) => {
      let latest: InvocationResult = {};
      for (let index = 0; index < callCount; index++) {
        latest = await underlying.invoke(['run'], unitValue, undefined);
      }
      return latest;
    });

    expect(invoke).toHaveBeenCalledTimes(callCount);
    expect(result.result).toBe(callCount === 0 ? undefined : unitValue);
  });

  it('rejects overlapping calls as SDK misuse', async () => {
    let resolve!: (result: InvocationResult) => void;
    const raw = {
      invoke: vi.fn(
        () =>
          new Promise<InvocationResult>((resolveInvocation) => {
            resolve = resolveInvocation;
          }),
      ),
    } as RawUnderlyingTool;

    await withInvocationScopedUnderlying(raw, undefined, async (underlying) => {
      const first = underlying.invoke(['first'], unitValue, undefined);
      await expect(underlying.invoke(['overlap'], unitValue, undefined)).rejects.toBeInstanceOf(
        ToolUnderlyingMisuseError,
      );
      resolve({});
      await first;
      return {};
    });

    expect(raw.invoke).toHaveBeenCalledOnce();
  });

  it('revokes the single wrapper when the middleware callback settles', async () => {
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;
    let escaped!: UniversalToolUnderlying;

    await withInvocationScopedUnderlying(raw, undefined, (underlying) => {
      escaped = underlying;
      return {};
    });

    await expect(escaped.invoke([], unitValue, undefined)).rejects.toMatchObject({
      name: 'ToolUnderlyingMisuseError',
      message: expect.stringContaining('no longer available'),
    });
    expect(raw.invoke).not.toHaveBeenCalled();
  });

  it('rejects an invocation queued after the callback returns before it reaches raw', async () => {
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;
    let lateInvocation!: Promise<InvocationResult>;

    await withInvocationScopedUnderlying(raw, undefined, (underlying) => {
      queueMicrotask(() => {
        lateInvocation = underlying.invoke([], unitValue, undefined);
        void lateInvocation.catch(() => undefined);
      });
      return {};
    });

    await expect(lateInvocation).rejects.toBeInstanceOf(ToolUnderlyingMisuseError);
    expect(raw.invoke).not.toHaveBeenCalled();
  });

  it('waits for a call initiated before a synchronous callback returns', async () => {
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;

    await withInvocationScopedUnderlying(raw, undefined, (underlying) => {
      void underlying.invoke([], unitValue, undefined);
      return {};
    });

    expect(raw.invoke).toHaveBeenCalledOnce();
  });

  it('revokes the wrapper and closes stdin when the middleware callback rejects', async () => {
    const stdin = controllableStream(1);
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;
    const rejection = new Error('middleware failed');
    let escaped!: UniversalToolUnderlying;

    await expect(
      withInvocationScopedUnderlying(raw, stdin.stream, (underlying) => {
        escaped = underlying;
        throw rejection;
      }),
    ).rejects.toBe(rejection);

    expect(stdin.close).toHaveBeenCalledOnce();
    await expect(escaped.invoke([], unitValue, undefined)).rejects.toBeInstanceOf(
      ToolUnderlyingMisuseError,
    );
  });

  it('closes unforwarded stdin on short circuit', async () => {
    const stdin = controllableStream(1, 2, 3);
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;

    await withInvocationScopedUnderlying(raw, stdin.stream, () => ({}));

    expect(stdin.close).toHaveBeenCalledOnce();
    expect(raw.invoke).not.toHaveBeenCalled();
  });

  it('transfers stdin once and permits a retry without stdin', async () => {
    const stdin = controllableStream(1, 2, 3);
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;

    await withInvocationScopedUnderlying(raw, stdin.stream, async (underlying) => {
      await underlying.invoke(['first'], unitValue, stdin.stream);
      await expect(underlying.invoke(['reuse'], unitValue, stdin.stream)).rejects.toMatchObject({
        name: 'ToolUnderlyingMisuseError',
        message: expect.stringContaining('already transferred'),
      });
      await underlying.invoke(['retry'], unitValue, undefined);
      return {};
    });

    expect(raw.invoke).toHaveBeenCalledTimes(2);
    expect(stdin.close).not.toHaveBeenCalled();
  });

  it('forwards stdout without consuming it and closes abandoned stdout once', async () => {
    const forwarded = controllableStream(4, 5, 6);
    const abandoned = controllableStream(7, 8, 9);
    const raw = {
      invoke: vi
        .fn<RawUnderlyingTool['invoke']>()
        .mockResolvedValueOnce({ stdout: forwarded.stream })
        .mockResolvedValueOnce({ stdout: abandoned.stream }),
    } as RawUnderlyingTool;

    const result = await withInvocationScopedUnderlying(raw, undefined, async (underlying) => {
      const selected = await underlying.invoke(['selected'], unitValue, undefined);
      await underlying.invoke(['abandoned'], unitValue, undefined);
      return selected;
    });

    expect(forwarded.close).not.toHaveBeenCalled();
    expect(abandoned.close).toHaveBeenCalledOnce();
    await result.stdout?.[Symbol.asyncIterator]().return?.();
    expect(forwarded.close).toHaveBeenCalledOnce();
  });

  it('closes stdout once when typed result validation fails', async () => {
    const stdout = controllableStream(1);
    const definition = toolDefinition('expected').body((body) =>
      body.stdout({ required: true }).returns(z.string()),
    );
    const raw = {
      invoke: vi.fn(async () => ({ result: wireValue(z.boolean(), true), stdout: stdout.stream })),
    } as RawUnderlyingTool;

    await withInvocationScopedUnderlying(raw, undefined, async (underlying) => {
      const client = createUnderlyingToolClient(definition, underlying);
      const failure = await rejectionOf(client.expected({}));
      expect(failure).toMatchObject({ cause: { tag: 'invalid-result' } });
      return {};
    });

    expect(stdout.close).toHaveBeenCalledOnce();
  });

  it('moves nested secret and quota handles into one raw invocation without cloning', async () => {
    const secret = { id: 'secret' } as never;
    const quota = { id: 'quota' } as never;
    const definition = toolDefinition('capabilities').body((body) =>
      body
        .positional('secret', s.secret(z.string()))
        .positional('quota', s.quotaToken())
        .returns(z.void()),
    );
    const seen: unknown[] = [];
    const raw = {
      invoke: vi.fn(async (_path, input) => {
        for (const node of input.value.valueNodes) {
          if (node.tag === 'secret-value' || node.tag === 'quota-token-handle') {
            seen.push(node.val);
          }
        }
        return {};
      }),
    } as RawUnderlyingTool;

    await withInvocationScopedUnderlying(raw, undefined, async (underlying) => {
      await createUnderlyingToolClient(definition, underlying).capabilities({ secret, quota });
      return {};
    });

    expect(seen).toEqual([secret, quota]);
    expect(raw.invoke).toHaveBeenCalledOnce();
  });

  it('validates and moves an owned quota result without consuming it during preflight', async () => {
    const quota = { id: 'result-quota' } as never;
    const schema = s.quotaToken();
    const definition = toolDefinition('quota-result').body((body) => body.returns(schema));
    const result = wireValue(schema, quota);
    const quotaNode = result.value.valueNodes.find((node) => node.tag === 'quota-token-handle');
    const raw = { invoke: vi.fn(async () => ({ result })) } as RawUnderlyingTool;

    await withInvocationScopedUnderlying(raw, undefined, async (underlying) => {
      await expect(
        createUnderlyingToolClient(definition, underlying)['quota-result']({}),
      ).resolves.toBe(quota);
      return {};
    });

    expect(quotaNode).toMatchObject({ tag: 'quota-token-handle', val: undefined });
    expect(raw.invoke).toHaveBeenCalledOnce();
  });
});
