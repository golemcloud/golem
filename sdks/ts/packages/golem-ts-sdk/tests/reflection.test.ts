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

import { describe, expect, it, vi } from 'vitest';
import {
  WasmRpc,
  getAgentType as hostGetAgentType,
  getAgentTypeByAgentId as hostGetAgentTypeByAgentId,
  parseAgentId,
  type RegisteredAgentType,
} from 'golem:agent/host@2.0.0';
import { SchemaRef } from '../src/schema/ref';
import {
  field,
  schemaGraphToWit,
  schemaValueToWit,
  t,
  v,
  type SchemaGraph,
} from '../src/internal/schema-model';
import { getAgentType, getAgentTypeByAgentId } from '../src/reflection';
import { RemoteCallError, RemoteOutputError } from '../src/client';
import { Uuid } from '../src/uuid';
import { ParsedAgentId } from '../src/agentId';

const stringGraph: SchemaGraph = { defs: new Map(), root: t.string() };

function registeredType(mode: 'durable' | 'ephemeral' = 'durable'): RegisteredAgentType {
  const schema = schemaGraphToWit(stringGraph);
  const metadata = { aliases: [], examples: [] };
  return {
    agentType: {
      typeName: 'ReflectedEcho',
      description: 'Echoes a string',
      sourceLanguage: 'typescript',
      schema,
      constructor: {
        description: 'Select an echo instance',
        inputSchema: {
          tag: 'parameters',
          val: [{ name: 'id', source: { tag: 'user-supplied' }, schema: schema.root, metadata }],
        },
      },
      methods: [
        {
          name: 'echo',
          description: 'Echo',
          httpEndpoint: [],
          inputSchema: {
            tag: 'parameters',
            val: [
              {
                name: 'message',
                source: { tag: 'user-supplied' },
                schema: schema.root,
                metadata,
              },
            ],
          },
          outputSchema: { tag: 'single', val: schema.root },
        },
      ],
      dependencies: [],
      mode,
      snapshotting: { tag: 'disabled' },
      config: [],
    },
    implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
  };
}

describe('SchemaRef', () => {
  const graph: SchemaGraph = {
    defs: new Map(),
    root: t.record([field('name', t.string()), field('count', t.u32())]),
  };
  const schema = new SchemaRef(graph);

  it('packs, validates, unpacks, and renders canonical JSON schema', () => {
    const packed = schema.packJson({ name: 'counter', count: 3 });
    expect(schema.validateValue(packed).success).toBe(true);
    expect(schema.unpackJson(packed)).toEqual({ name: 'counter', count: 3 });
    expect(schema.toJsonSchema()).toMatchObject({
      $schema: 'https://json-schema.org/draft/2020-12/schema',
      type: 'object',
      required: ['name', 'count'],
    });
  });

  it('returns a structured path for invalid JSON', () => {
    expect(schema.validateJson({ name: 'counter', count: -1 })).toEqual({
      success: false,
      issues: [{ path: ['count'], message: expect.stringContaining('outside') }],
    });
  });

  it('does not expose mutable graph definitions', () => {
    expect(() =>
      (schema.graph.defs as Map<string, unknown>).set('new-type', {
        body: t.string(),
      }),
    ).toThrow('immutable schema graph');
  });

  it('deep-clones caller-owned schema nodes before freezing', () => {
    const root = t.record([field('value', t.string())]);
    const definition = { name: 'Shared', body: t.string() };
    const owned: SchemaGraph = { defs: new Map([['shared', definition]]), root };
    const reflected = new SchemaRef(owned);

    (root.body as Extract<typeof root.body, { tag: 'record' }>).fields[0].name = 'changed';
    definition.name = 'Changed';
    owned.defs.set('later', { name: 'Later', body: t.bool() });

    expect(
      (reflected.root.body as Extract<typeof reflected.root.body, { tag: 'record' }>).fields[0]
        .name,
    ).toBe('value');
    expect(reflected.graph.defs.get('shared')?.name).toBe('Shared');
    expect(reflected.graph.defs.has('later')).toBe(false);
  });
});

describe('agent reflection', () => {
  it('returns undefined when an optional type lookup misses', () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(undefined);
    expect(getAgentType('Missing')).toBeUndefined();
  });

  it('rejects malformed reflected schema graphs', () => {
    const malformed = registeredType();
    const output = malformed.agentType.methods[0].outputSchema;
    if (output.tag !== 'single') throw new Error('test agent must declare a single output');
    output.val = malformed.agentType.schema.typeNodes.length;
    vi.mocked(hostGetAgentType).mockReturnValueOnce(malformed);
    expect(() => getAgentType('ReflectedEcho')).toThrow(/type node index out of range/);
  });

  it('rejects malformed constructor values and agent IDs', () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    const reflected = getAgentType('ReflectedEcho')!;
    expect(() => reflected.client.get({ id: 1 })).toThrow(/expected .*string/);

    vi.mocked(parseAgentId).mockImplementationOnce(() => {
      throw new TypeError('malformed agent id');
    });
    const malformed = new ParsedAgentId('not-an-agent-id');
    expect(() => malformed.parts()).toThrow('malformed agent id');
  });

  it('discovers a type and invokes through its reflected schemas', async () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    const reflected = getAgentType('ReflectedEcho')!;
    const client = reflected.client.get({ id: 'one' });
    const rpc = vi.mocked(WasmRpc.create).mock.results.at(-1)!.value;
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: { agentId: 'ReflectedEcho(one)', idempotencyKey: 'key' },
      future: {
        get: vi.fn().mockResolvedValue(schemaValueToWit(v.string('hello'))),
        cancel: vi.fn(),
      },
    });

    await expect(client.method('echo').invokeJson({ message: 'hello' })).resolves.toEqual({
      metadata: { agentId: 'ReflectedEcho(one)', idempotencyKey: 'key' },
      value: 'hello',
    });
  });

  it('decodes a reflected graph once and shares immutable definitions and type nodes', () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    const reflected = getAgentType('ReflectedEcho')!;
    const method = reflected.method('echo')!;
    const constructorField = (
      reflected.constructorInput.root.body as Extract<
        typeof reflected.constructorInput.root.body,
        { tag: 'record' }
      >
    ).fields[0];
    const methodField = (
      method.input.root.body as Extract<typeof method.input.root.body, { tag: 'record' }>
    ).fields[0];

    expect(reflected.constructorInput.graph.defs).toBe(method.input.graph.defs);
    expect(method.input.graph.defs).toBe(method.output!.graph.defs);
    expect(constructorField.body).toBe(methodField.body);
    expect(methodField.body).toBe(method.output!.root);
    expect(() =>
      (method.output!.graph.defs as Map<string, unknown>).set('new', t.string()),
    ).toThrow('immutable schema graph');
  });

  it('rejects missing and malformed values for a declared output', async () => {
    vi.mocked(hostGetAgentType).mockReturnValue(registeredType());
    const reflected = getAgentType('ReflectedEcho')!;
    const client = reflected.client.get({ id: 'one' });
    const rpc = vi.mocked(WasmRpc.create).mock.results.at(-1)!.value;

    rpc.asyncInvokeAndAwait.mockReturnValueOnce({
      metadata: { agentId: 'ReflectedEcho(one)', idempotencyKey: 'missing' },
      future: { get: vi.fn().mockResolvedValue(undefined), cancel: vi.fn() },
    });
    await expect(client.method('echo').invokeValue(v.record([]))).rejects.toBeInstanceOf(
      RemoteOutputError,
    );

    rpc.asyncInvokeAndAwait.mockReturnValueOnce({
      metadata: { agentId: 'ReflectedEcho(one)', idempotencyKey: 'malformed' },
      future: { get: vi.fn().mockResolvedValue(schemaValueToWit(v.u32(1))), cancel: vi.fn() },
    });
    await expect(client.method('echo').invokeValue(v.record([]))).rejects.toBeInstanceOf(
      RemoteOutputError,
    );
  });

  it('looks up the current schema for a concrete agent instance', () => {
    const rawId = new ParsedAgentId('ReflectedEcho(one)');
    vi.mocked(hostGetAgentTypeByAgentId).mockReturnValueOnce(registeredType());

    expect(getAgentTypeByAgentId(rawId)?.name).toBe('ReflectedEcho');
    expect(hostGetAgentTypeByAgentId).toHaveBeenLastCalledWith(rawId.value);
  });

  it('creates a bare client without a discovery lookup', async () => {
    vi.mocked(parseAgentId).mockReturnValueOnce([
      'ReflectedEcho',
      {
        graph: schemaGraphToWit(stringGraph),
        value: schemaValueToWit(v.record([v.string('one')])),
      },
      undefined,
    ]);
    const before = vi.mocked(hostGetAgentType).mock.calls.length;
    const agentId = new ParsedAgentId('ReflectedEcho(one)');
    const client = agentId.dynamicClient();
    const rpc = vi.mocked(WasmRpc.create).mock.results.at(-1)!.value;
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: { agentId: 'ReflectedEcho(one)', idempotencyKey: 'key' },
      future: {
        get: vi.fn().mockResolvedValue(schemaValueToWit(v.string('hello'))),
        cancel: vi.fn(),
      },
    });

    await expect(client.method('anything').invokeValue(v.record([]))).resolves.toMatchObject({
      value: v.string('hello'),
    });
    expect(hostGetAgentType).toHaveBeenCalledTimes(before);
  });

  it('binds a discovered reflected type fluently to an existing identity', () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    vi.mocked(parseAgentId).mockReturnValueOnce([
      'ReflectedEcho',
      {
        graph: schemaGraphToWit(stringGraph),
        value: schemaValueToWit(v.record([v.string('one')])),
      },
      undefined,
    ]);
    const reflected = getAgentType('ReflectedEcho')!;
    const agentId = new ParsedAgentId('ReflectedEcho(one)');

    const client = agentId.client(reflected);

    expect(client).not.toHaveProperty('agentId');
    expect(client.method('echo').definition.name).toBe('echo');
  });

  it('rejects binding a reflected ephemeral type to an existing identity', () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType('ephemeral'));
    vi.mocked(parseAgentId).mockReturnValueOnce([
      'ReflectedEcho',
      {
        graph: schemaGraphToWit(stringGraph),
        value: schemaValueToWit(v.record([v.string('one')])),
      },
      undefined,
    ]);
    const reflected = getAgentType('ReflectedEcho')!;
    const agentId = new ParsedAgentId('ReflectedEcho(one)');
    const creates = vi.mocked(WasmRpc.create).mock.calls.length;

    expect(() => agentId.client(reflected)).toThrow(
      "Cannot bind existing ParsedAgentId 'ReflectedEcho(one)' to ephemeral agent type 'ReflectedEcho'; use agentType.client.newPhantom(...)",
    );
    expect(WasmRpc.create).toHaveBeenCalledTimes(creates);
  });

  it('preserves the structured RPC error returned by client creation', () => {
    const rpcError = { tag: 'not-found' as const, val: 'missing deployment' };
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    vi.mocked(WasmRpc.create).mockImplementationOnce(() => {
      throw rpcError;
    });

    try {
      getAgentType('ReflectedEcho')!.client.get({ id: 'one' });
      throw new Error('expected client creation to fail');
    } catch (error) {
      expect(error).toBeInstanceOf(RemoteCallError);
      expect(error).toMatchObject({
        cause: { tag: 'not-found', details: rpcError.val },
      });
    }
  });

  it('returns invocation metadata without synthetic identity for ephemeral reflected clients', async () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType('ephemeral'));
    const reflected = getAgentType('ReflectedEcho')!;
    const phantomId = new Uuid(1n, 2n);
    const known = reflected.client.getPhantom({ id: 'one' }, phantomId);
    const fresh = reflected.client.newPhantom({ id: 'two' });
    const rpc = vi.mocked(WasmRpc.create).mock.results.at(-1)!.value;
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: { agentId: 'ReflectedEcho(two)[final]', idempotencyKey: 'key' },
      future: {
        get: vi.fn().mockResolvedValue(schemaValueToWit(v.string('hello'))),
        cancel: vi.fn(),
      },
    });

    expect(reflected.agentId({ id: 'one' }, phantomId).value).toBe('MockAgent()');
    expect(known).not.toHaveProperty('agentId');
    if ('client' in fresh) throw new Error('ephemeral newPhantom returned a durable wrapper');
    expect(fresh).not.toHaveProperty('agentId');
    await expect(fresh.method('echo').invoke({ message: 'hello' })).resolves.toEqual({
      metadata: { agentId: 'ReflectedEcho(two)[final]', idempotencyKey: 'key' },
      value: 'hello',
    });
  });
});
