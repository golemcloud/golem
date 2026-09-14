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
  TypedSchemaValue,
  UnderlyingTool,
} from 'golem:tool/common@0.1.0';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod/v4';
import {
  golemTool010ToolMiddlewareGuest as combinedGuest,
  toolMiddlewareGuest as combinedGuestAlias,
} from '../src/index';
import {
  golemTool010ToolMiddlewareGuest as pureGuest,
  toolMiddlewareGuest as pureGuestAlias,
} from '../src/middlewareRuntime';
import { compileSchema } from '../src/schema/adapter';
import { typedSchemaValueFromWit, typedSchemaValueToWit } from '../src/internal/schema-model';
import { ToolMiddlewareRegistry } from '../src/internal/registry/toolMiddlewareRegistry';
import { ToolRegistry } from '../src/internal/registry/toolRegistry';
import {
  ToolInvokeError,
  toolDefinition,
  universalToolMiddleware,
  type ToolImplementation,
} from '../src/tool';

type RawUnderlyingTool = Pick<UnderlyingTool, 'invoke'>;

beforeEach(() => {
  ToolMiddlewareRegistry.clearForTests();
  ToolRegistry.clearForTests();
});

describe('tool middleware guest exports', () => {
  it('exports the short WIT interface name from both runtime entries', () => {
    expect(combinedGuestAlias).toBe(combinedGuest);
    expect(pureGuestAlias).toBe(pureGuest);
  });
});

function wireValue(schema: Parameters<typeof compileSchema>[0], value: unknown): TypedSchemaValue {
  const codec = compileSchema(schema);
  return typedSchemaValueToWit({ graph: codec.graph, value: codec.toValue(value) });
}

function decodeValue(
  schema: Parameters<typeof compileSchema>[0],
  value: TypedSchemaValue,
): unknown {
  const codec = compileSchema(schema);
  return codec.fromValue(typedSchemaValueFromWit(value).value);
}

function rawTool(): Tool {
  return {
    version: '1.0.0',
    commands: { nodes: [] },
    schema: { typeNodes: [], defs: [], root: 0 },
  } as Tool;
}

const anonymous = { tag: 'anonymous' } as const;

describe('tool middleware registry and guest boundary', () => {
  it('exports one guest object through the pure and combined runtime entries', () => {
    expect(pureGuest).toBe(combinedGuest);
  });

  it('discovers canonical names in sorted order with complete scopes', () => {
    const transparent = toolDefinition('presented')
      .version('2.0.0')
      .body((body) => body.returns(z.string()));
    transparent.middleware({
      name: 'z-policy',
      aliases: ['z-alias'],
      doc: 'Transparent policy',
      implementation: { presented: async () => 'value' },
    });
    universalToolMiddleware({
      name: 'a-policy',
      aliases: ['a-alias'],
      invoke: async () => ({}),
    });

    const discovered = combinedGuest.discoverToolMiddlewares();

    expect(discovered.map(({ name }) => name)).toEqual(['a-policy', 'z-policy']);
    expect(discovered[0]).toMatchObject({
      name: 'a-policy',
      aliases: ['a-alias'],
      scope: { tag: 'universal' },
    });
    expect(discovered[0].scope).not.toHaveProperty('val');
    expect(discovered[1]).toMatchObject({
      name: 'z-policy',
      aliases: ['z-alias'],
      doc: { summary: 'Transparent policy' },
      scope: {
        tag: 'monomorphic',
        val: {
          presented: { version: '2.0.0' },
          expected: { version: '2.0.0' },
        },
      },
    });
    if (discovered[1].scope.tag !== 'monomorphic') throw new Error('expected monomorphic scope');
    expect(discovered[1].scope.val.expected).toEqual(discovered[1].scope.val.presented);
    expect(discovered[1].scope.val.presented.commands.nodes.map(({ name }) => name)).toEqual([
      'presented',
    ]);
  });

  it('looks up only canonical names and keeps the ordinary namespace independent', () => {
    universalToolMiddleware({
      name: 'shared-name',
      aliases: ['middleware-alias'],
      invoke: async () => ({}),
    });
    toolDefinition('shared-name')
      .body((body) => body.returns(z.void()))
      .implement({ 'shared-name': async () => ({ tag: 'ok', value: undefined }) });

    expect(combinedGuest.getToolMiddleware('shared-name').name).toBe('shared-name');
    expect(ToolRegistry.get('shared-name')).toBeDefined();
    expect(() => combinedGuest.getToolMiddleware('middleware-alias')).toThrowError();
    try {
      combinedGuest.getToolMiddleware('middleware-alias');
    } catch (error) {
      expect(error).toEqual({ tag: 'invalid-tool-name', val: 'middleware-alias' });
    }
  });

  it('invokes universal and monomorphic entries and returns wire errors', async () => {
    const input = wireValue(z.string(), 'input');
    const observed = vi.fn();
    universalToolMiddleware({
      name: 'universal-boundary',
      invoke: async (request, { underlying }) => {
        observed(request.toolName, request.toolMetadata, request.principal);
        return underlying.invoke(request.commandPath, request.input, request.stdin);
      },
    });
    const raw = { invoke: vi.fn(async () => ({ result: input })) } as RawUnderlyingTool;

    const universalResult = await combinedGuest.invokeToolMiddleware(
      'universal-boundary',
      'runtime-tool',
      rawTool(),
      ['run'],
      input,
      undefined,
      anonymous,
      raw,
    );

    expect(universalResult.result).toBe(input);
    expect(observed).toHaveBeenCalledWith('runtime-tool', expect.anything(), { tag: 'anonymous' });

    const monomorphic = toolDefinition('presented').body((body) => body.returns(z.string()));
    monomorphic.middleware({
      name: 'monomorphic-boundary',
      implementation: { presented: async () => 'short-circuit' },
    });
    const monomorphicResult = await combinedGuest.invokeToolMiddleware(
      'monomorphic-boundary',
      'ignored-runtime-name',
      rawTool(),
      [],
      wireValue(z.object({}), {}),
      undefined,
      anonymous,
      { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool,
    );
    expect(decodeValue(z.string(), monomorphicResult.result!)).toBe('short-circuit');

    universalToolMiddleware({
      name: 'wire-custom-error',
      invoke: async (request) => {
        throw ToolInvokeError.tool(request.input);
      },
    });
    await expect(
      combinedGuest.invokeToolMiddleware(
        'wire-custom-error',
        'runtime-tool',
        rawTool(),
        [],
        input,
        undefined,
        anonymous,
        raw,
      ),
    ).rejects.toEqual({ tag: 'custom-error', val: input });

    universalToolMiddleware({
      name: 'wire-protocol-error',
      invoke: async () => {
        throw new ToolInvokeError({ tag: 'constraint-violation', val: 'denied' });
      },
    });
    await expect(
      combinedGuest.invokeToolMiddleware(
        'wire-protocol-error',
        'runtime-tool',
        rawTool(),
        [],
        input,
        undefined,
        anonymous,
        raw,
      ),
    ).rejects.toEqual({ tag: 'constraint-violation', val: 'denied' });
  });

  it('rejects an unknown name before converting principal or touching wrapped', async () => {
    const wrapped = {} as RawUnderlyingTool;
    Object.defineProperty(wrapped, 'invoke', {
      get() {
        throw new Error('wrapped was touched');
      },
    });

    await expect(
      combinedGuest.invokeToolMiddleware(
        'missing',
        'runtime-tool',
        null as never,
        [],
        null as never,
        undefined,
        null as never,
        wrapped,
      ),
    ).rejects.toEqual({ tag: 'invalid-tool-name', val: 'missing' });
  });

  it('reserves names during reentrant registration and retains the outer atomic entry', () => {
    const outerHandler = vi.fn(async () => undefined);
    const innerHandler = vi.fn(async () => undefined);
    const outer = toolDefinition('presented').body((body) => body.returns(z.void()));
    const inner = toolDefinition('presented').body((body) => body.returns(z.void()));
    let entered = false;
    const implementation = {} as ToolImplementation<typeof outer>;
    Object.defineProperty(implementation, 'presented', {
      enumerable: true,
      get() {
        if (!entered) {
          entered = true;
          inner.middleware({
            name: 'reentrant-policy',
            implementation: { presented: innerHandler },
          });
        }
        return outerHandler;
      },
    });

    outer.middleware({ name: 'reentrant-policy', implementation });

    const source = ToolMiddlewareRegistry.getSource('reentrant-policy');
    expect(source?.kind).toBe('monomorphic');
    if (source?.kind !== 'monomorphic') throw new Error('missing monomorphic source');
    expect(source.runtime.bindings[0]?.handler).toBe(outerHandler);
    expect(ToolMiddlewareRegistry.getRegistrationErrors()).toEqual([
      {
        name: 'reentrant-policy',
        messages: [expect.stringContaining('already registered')],
      },
    ]);
  });

  it('surfaces sorted deferred diagnostics and never executes a valid callback', async () => {
    const callback = vi.fn(async () => ({}));
    universalToolMiddleware({ name: 'valid-policy', invoke: callback });
    universalToolMiddleware({ name: 'valid-policy', invoke: callback });
    universalToolMiddleware({ name: 'Invalid', invoke: callback });
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;

    expect(() => combinedGuest.discoverToolMiddlewares()).toThrowError();
    let discoveryError: unknown;
    try {
      combinedGuest.discoverToolMiddlewares();
    } catch (error) {
      discoveryError = error;
    }
    expect(discoveryError).toMatchObject({ tag: 'invalid-result' });
    const message = (discoveryError as { val: string }).val;
    expect(message.indexOf('"Invalid"')).toBeLessThan(message.indexOf('"valid-policy"'));

    await expect(
      combinedGuest.invokeToolMiddleware(
        'valid-policy',
        'runtime-tool',
        rawTool(),
        [],
        wireValue(z.string(), 'input'),
        undefined,
        anonymous,
        raw,
      ),
    ).rejects.toMatchObject({ tag: 'invalid-result' });
    expect(callback).not.toHaveBeenCalled();
    expect(raw.invoke).not.toHaveBeenCalled();
  });

  it('copies the selected invoker before awaiting user code', async () => {
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const started = vi.fn();
    universalToolMiddleware({
      name: 'stable-selection',
      invoke: async () => {
        started();
        await gate;
        return {};
      },
    });

    const pending = combinedGuest.invokeToolMiddleware(
      'stable-selection',
      'runtime-tool',
      rawTool(),
      [],
      wireValue(z.string(), 'input'),
      undefined,
      anonymous,
      { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool,
    );
    await vi.waitFor(() => expect(started).toHaveBeenCalledOnce());
    ToolMiddlewareRegistry.clearForTests();
    release();

    await expect(pending).resolves.toEqual({});
  });

  it('does not partially insert a failed implementation', () => {
    const definition = toolDefinition('presented').body((body) => body.returns(z.void()));
    definition.middleware({
      name: 'atomic-failure',
      implementation: {} as never,
    });

    expect(ToolMiddlewareRegistry.get('atomic-failure')).toBeUndefined();
    expect(ToolMiddlewareRegistry.getRegistrationErrors()).toEqual([
      {
        name: 'atomic-failure',
        messages: [expect.stringContaining('implementation must be a function')],
      },
    ]);
  });
});
