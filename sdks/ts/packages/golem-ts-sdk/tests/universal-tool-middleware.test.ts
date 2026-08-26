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
  Tool,
  ToolError,
  TypedSchemaValue,
  UnderlyingTool,
} from 'golem:tool/common@0.1.0';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod/v4';
import { ToolMiddlewareRegistry } from '../src/internal/registry/toolMiddlewareRegistry';
import {
  invokeUniversalToolMiddleware,
  ToolUnderlyingMisuseError,
} from '../src/internal/tool/middlewareRuntime';
import { typedSchemaValueToWit } from '../src/internal/schema-model';
import { sdkPrincipalFromHost } from '../src/principal';
import { compileSchema } from '../src/schema/adapter';
import { s } from '../src/schema/markers';
import {
  ToolInvokeError,
  universalToolMiddleware,
  type UniversalToolMiddlewareInvocation,
  type UniversalToolUnderlying,
} from '../src/tool';

type RawUnderlyingTool = Pick<UnderlyingTool, 'invoke'>;

beforeEach(() => {
  ToolMiddlewareRegistry.clearForTests();
});

function wireValue(schema: Parameters<typeof compileSchema>[0], value: unknown): TypedSchemaValue {
  const codec = compileSchema(schema);
  return typedSchemaValueToWit({ graph: codec.graph, value: codec.toValue(value) });
}

function universalSource(name: string) {
  const source = ToolMiddlewareRegistry.getSource(name);
  if (!source || source.kind !== 'universal') {
    throw new Error(`missing universal middleware ${name}`);
  }
  return source;
}

function invoke(
  name: string,
  invocation: UniversalToolMiddlewareInvocation,
  raw: RawUnderlyingTool,
): Promise<InvocationResult> {
  return invokeUniversalToolMiddleware(universalSource(name), invocation, raw);
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

function invocation(
  overrides: Partial<UniversalToolMiddlewareInvocation> = {},
): UniversalToolMiddlewareInvocation {
  return {
    toolName: 'runtime-tool',
    toolMetadata: { raw: 'metadata' } as unknown as Tool,
    commandPath: ['nested', 'run'],
    input: wireValue(z.string(), 'input'),
    principal: sdkPrincipalFromHost({ tag: 'anonymous' }),
    ...overrides,
  };
}

describe('universal tool middleware dispatch', () => {
  it('rejects malformed outer input as invalid-input before invoking universal middleware', async () => {
    const callback = vi.fn(async () => ({}));
    universalToolMiddleware({
      name: 'raw-malformed-input',
      invoke: callback,
    });
    const malformed = {
      graph: { typeNodes: [], defs: [], root: 0 },
      value: { valueNodes: [], root: 0 },
    } as TypedSchemaValue;
    const stdin = controllableStream(1);
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;

    await expect(
      invoke('raw-malformed-input', invocation({ input: malformed, stdin: stdin.stream }), raw),
    ).rejects.toMatchObject({ cause: { tag: 'invalid-input' } });
    expect(callback).not.toHaveBeenCalled();
    expect(raw.invoke).not.toHaveBeenCalled();
    expect(stdin.close).toHaveBeenCalledOnce();
  });

  it('registers normalized metadata with universal scope and no static shape', () => {
    const implementation = vi.fn(async () => ({}));
    expect(
      universalToolMiddleware({
        name: 'universal-audit',
        aliases: ['audit'],
        doc: 'Audits raw calls',
        invoke: implementation,
      }),
    ).toEqual({ name: 'universal-audit' });

    expect(universalSource('universal-audit')).toMatchObject({
      kind: 'universal',
      name: 'universal-audit',
      aliases: ['audit'],
      doc: { summary: 'Audits raw calls', description: '', examples: [] },
      invoke: implementation,
    });
    expect(universalSource('universal-audit')).not.toHaveProperty('presented');
    expect(universalSource('universal-audit')).not.toHaveProperty('expected');
  });

  it('forwards raw metadata, carriers, principal, stdin, result, and stdout semantically unchanged', async () => {
    let observed: UniversalToolMiddlewareInvocation | undefined;
    universalToolMiddleware({
      name: 'raw-forward',
      invoke: async (request, { underlying }) => {
        observed = request;
        return underlying.invoke(request.commandPath, request.input, request.stdin);
      },
    });
    const stdin = controllableStream(1, 2);
    const stdout = controllableStream(3, 4);
    const request = invocation({ stdin: stdin.stream });
    const rawResult = { result: request.input, stdout: stdout.stream };
    const raw = {
      invoke: vi.fn(async (path, input, receivedStdin) => {
        expect(path).toEqual(request.commandPath);
        expect(input).toBe(request.input);
        expect(receivedStdin).toBe(stdin.stream);
        return rawResult;
      }),
    } as RawUnderlyingTool;

    const result = await invoke('raw-forward', request, raw);

    expect(observed).toBe(request);
    expect(observed?.toolName).toBe('runtime-tool');
    expect(observed?.toolMetadata).toBe(request.toolMetadata);
    expect(observed?.principal).toBe(request.principal);
    expect(result.result).toBe(request.input);
    expect(result.stdout).toBeDefined();
    expect(stdin.close).not.toHaveBeenCalled();
    expect(stdout.close).not.toHaveBeenCalled();
    await result.stdout?.[Symbol.asyncIterator]().return?.();
    expect(stdout.close).toHaveBeenCalledOnce();
  });

  it('short-circuits and replaces a result while closing unforwarded stdin', async () => {
    const replacement = wireValue(z.string(), 'replacement');
    universalToolMiddleware({
      name: 'raw-short-circuit',
      invoke: async () => ({ result: replacement }),
    });
    const stdin = controllableStream(1);
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;

    const result = await invoke('raw-short-circuit', invocation({ stdin: stdin.stream }), raw);

    expect(result.result).toBe(replacement);
    expect(raw.invoke).not.toHaveBeenCalled();
    expect(stdin.close).toHaveBeenCalledOnce();
  });

  it('transfers underlying stdout into a later underlying stdin without closing it', async () => {
    const stdout = controllableStream(1, 2);
    let receivedStdin: AsyncIterable<number> | undefined;
    universalToolMiddleware({
      name: 'raw-stream-chain',
      invoke: async (request, { underlying }) => {
        const first = await underlying.invoke(['first'], request.input, undefined);
        await underlying.invoke(['second'], request.input, first.stdout);
        return {};
      },
    });
    const raw = {
      invoke: vi
        .fn<RawUnderlyingTool['invoke']>()
        .mockResolvedValueOnce({ stdout: stdout.stream })
        .mockImplementationOnce(async (_path, _input, stdin) => {
          receivedStdin = stdin;
          return {};
        }),
    } as RawUnderlyingTool;

    await invoke('raw-stream-chain', invocation(), raw);

    expect(receivedStdin).toBeDefined();
    expect(stdout.close).not.toHaveBeenCalled();
    await receivedStdin?.[Symbol.asyncIterator]().return?.();
    expect(stdout.close).toHaveBeenCalledOnce();
  });

  it('transfers outer stdin into final stdout without closing it', async () => {
    universalToolMiddleware({
      name: 'raw-stream-reverse',
      invoke: async (request) => ({ stdout: request.stdin }),
    });
    const stdin = controllableStream(1, 2);

    const result = await invoke('raw-stream-reverse', invocation({ stdin: stdin.stream }), {
      invoke: vi.fn(async () => ({})),
    } as RawUnderlyingTool);

    expect(stdin.close).not.toHaveBeenCalled();
    await result.stdout?.[Symbol.asyncIterator]().return?.();
    expect(stdin.close).toHaveBeenCalledOnce();
  });

  it('rejects returning a stream after transferring it to underlying', async () => {
    universalToolMiddleware({
      name: 'raw-stream-reuse',
      invoke: async (request, { underlying }) => {
        await underlying.invoke(request.commandPath, request.input, request.stdin);
        return { stdout: request.stdin };
      },
    });
    const stdin = controllableStream(1, 2);
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;

    await expect(
      invoke('raw-stream-reuse', invocation({ stdin: stdin.stream }), raw),
    ).rejects.toBeInstanceOf(ToolUnderlyingMisuseError);
    expect(raw.invoke).toHaveBeenCalledOnce();
    expect(stdin.close).not.toHaveBeenCalled();
    await stdin.stream[Symbol.asyncIterator]().return?.();
    expect(stdin.close).toHaveBeenCalledOnce();
  });

  it('preserves every protocol error and the raw custom payload', async () => {
    universalToolMiddleware({
      name: 'raw-errors',
      invoke: (request, { underlying }) =>
        underlying.invoke(request.commandPath, request.input, request.stdin),
    });
    const customPayload = wireValue(z.string(), 'custom');
    const cases: ToolError[] = [
      { tag: 'invalid-tool-name', val: 'wrong-tool' },
      { tag: 'invalid-command-path', val: ['wrong', 'path'] },
      { tag: 'invalid-input', val: 'bad-input' },
      { tag: 'constraint-violation', val: 'constraint' },
      { tag: 'invalid-result', val: 'bad-result' },
      { tag: 'custom-error', val: customPayload },
    ];

    for (const wireError of cases) {
      const raw = {
        invoke: vi.fn(async () => {
          throw wireError;
        }),
      } as RawUnderlyingTool;
      const error = (await rejectionOf(invoke('raw-errors', invocation(), raw))) as ToolInvokeError;
      expect(error).toBeInstanceOf(ToolInvokeError);
      expect(error.cause).toEqual(
        wireError.tag === 'custom-error' ? { tag: 'tool', error: customPayload } : wireError,
      );
      if (wireError.tag === 'custom-error' && error.cause.tag === 'tool') {
        expect(error.cause.error).toBe(customPayload);
      }
    }
  });

  it('allows multiple sequential raw calls and no underlying call', async () => {
    let callCount = 0;
    universalToolMiddleware({
      name: 'raw-control-flow',
      invoke: async (request, { underlying }) => {
        if (request.commandPath[0] === 'zero') return {};
        await underlying.invoke(['first'], request.input, undefined);
        return underlying.invoke(['second'], request.input, undefined);
      },
    });
    const raw = {
      invoke: vi.fn(async (_path, input) => {
        callCount++;
        return { result: input };
      }),
    } as RawUnderlyingTool;

    await expect(
      invoke('raw-control-flow', invocation({ commandPath: ['zero'] }), raw),
    ).resolves.toEqual({});
    expect(callCount).toBe(0);

    const request = invocation({ commandPath: ['multiple'] });
    const result = await invoke('raw-control-flow', request, raw);
    expect(callCount).toBe(2);
    expect(result.result).toBe(request.input);
  });

  it('enforces overlap and revocation through the universal entry', async () => {
    let release!: () => void;
    let escaped: UniversalToolUnderlying | undefined;
    let overlap: Promise<unknown> | undefined;
    universalToolMiddleware({
      name: 'raw-lifecycle',
      invoke: async (request, { underlying }) => {
        escaped = underlying;
        const first = underlying.invoke(request.commandPath, request.input, undefined);
        overlap = rejectionOf(underlying.invoke(request.commandPath, request.input, undefined));
        release();
        await first;
        return {};
      },
    });
    let finish!: () => void;
    const started = new Promise<void>((resolve) => {
      release = resolve;
    });
    const raw = {
      invoke: vi.fn(
        async () =>
          new Promise<InvocationResult>((resolve) => {
            finish = () => resolve({});
          }),
      ),
    } as RawUnderlyingTool;

    const pending = invoke('raw-lifecycle', invocation(), raw);
    await started;
    finish();
    await pending;

    await expect(overlap).resolves.toBeInstanceOf(ToolUnderlyingMisuseError);
    await expect(
      escaped!.invoke([], wireValue(z.string(), 'late'), undefined),
    ).rejects.toBeInstanceOf(ToolUnderlyingMisuseError);
    expect(raw.invoke).toHaveBeenCalledOnce();
  });

  it('passes nested affine carriers once without reconstructing handles', async () => {
    const quota = { id: 'quota' } as never;
    const input = wireValue(s.quotaToken(), quota);
    const quotaNode = input.value.valueNodes.find((node) => node.tag === 'quota-token-handle');
    universalToolMiddleware({
      name: 'raw-affine',
      invoke: (request, { underlying }) =>
        underlying.invoke(request.commandPath, request.input, undefined),
    });
    const raw = {
      invoke: vi.fn(async (_path, received) => {
        expect(received).toBe(input);
        expect(received.value.valueNodes.find((node) => node.tag === 'quota-token-handle')).toBe(
          quotaNode,
        );
        return { result: received };
      }),
    } as RawUnderlyingTool;

    const result = await invoke('raw-affine', invocation({ input }), raw);

    expect(result.result).toBe(input);
    expect(quotaNode).toMatchObject({ tag: 'quota-token-handle', val: quota });
  });

  it('maps malformed final results and raw custom payloads to invalid-result', async () => {
    const malformed = {
      graph: { typeNodes: [], defs: [], root: 0 },
      value: { valueNodes: [], root: 0 },
    } as TypedSchemaValue;
    let mode: 'result' | 'custom' = 'result';
    universalToolMiddleware({
      name: 'raw-invalid',
      invoke: async () => {
        if (mode === 'custom') throw ToolInvokeError.tool(malformed);
        return null as never;
      },
    });
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;

    await expect(invoke('raw-invalid', invocation(), raw)).rejects.toMatchObject({
      cause: { tag: 'invalid-result' },
    });
    mode = 'custom';
    await expect(invoke('raw-invalid', invocation(), raw)).rejects.toMatchObject({
      cause: { tag: 'invalid-result' },
    });
  });

  it('does not reinterpret ordinary JavaScript exceptions', async () => {
    const failure = new Error('implementation bug');
    universalToolMiddleware({
      name: 'raw-trap',
      invoke: async () => {
        throw failure;
      },
    });

    await expect(
      invoke('raw-trap', invocation(), { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool),
    ).rejects.toBe(failure);
  });
});
