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
import { AgentId } from '../src/agentId';
import { DynamicAgentClient } from '../src/dynamicClient';
import { hostInvocationMetadata, sdkInvocationMetadata } from './invocationMetadataTestSupport';
import { MOCK_REMOTE_AGENT_ID } from './testSetup';

const ECHO_AGENT_ID = AgentId.parse('00000007-0000-8000-8000-000000000011');

const stringGraph: SchemaGraph = { defs: new Map(), root: t.string() };

function registeredType(mode: 'durable' | 'ephemeral' = 'durable'): RegisteredAgentType {
  const schema = schemaGraphToWit(stringGraph);
  const metadata = { aliases: [], examples: [] };
  return {
    agentType: {
      typeName: 'ReflectedEcho',
      kind: 'regular',
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

  it('rejects malformed constructor values before opening an RPC client', async () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    const reflected = getAgentType('ReflectedEcho')!;
    const upserts = vi.mocked(WasmRpc.upsert).mock.calls.length;
    const creates = vi.mocked(WasmRpc.create).mock.calls.length;
    expect(() => reflected.client.upsert({ id: 1 })).toThrow(/expected .*string/);
    await expect(reflected.client.create({ id: 1 })).rejects.toThrow(/expected .*string/);
    expect(WasmRpc.upsert).toHaveBeenCalledTimes(upserts);
    expect(WasmRpc.create).toHaveBeenCalledTimes(creates);
  });

  it('maps a reflected upsert to the host upsert with the packed constructor value', () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    getAgentType('ReflectedEcho')!.client.upsert({ id: 'one' });

    const call = vi.mocked(WasmRpc.upsert).mock.calls.at(-1)!;
    expect(call).toHaveLength(2);
    expect(call[0]).toBe('ReflectedEcho');
    expect(call[1]).toEqual(schemaValueToWit(v.record([v.string('one')])));
  });

  it('discovers a type and invokes through its reflected schemas', async () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    const reflected = getAgentType('ReflectedEcho')!;
    const client = reflected.client.upsert({ id: 'one' });
    const rpc = vi.mocked(WasmRpc.upsert).mock.results.at(-1)!.value;
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: hostInvocationMetadata(),
      future: {
        get: vi.fn().mockResolvedValue(schemaValueToWit(v.string('hello'))),
        cancel: vi.fn(),
      },
    });

    await expect(client.method('echo').invokeJson({ message: 'hello' })).resolves.toEqual({
      metadata: sdkInvocationMetadata(),
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
    const client = reflected.client.upsert({ id: 'one' });
    const rpc = vi.mocked(WasmRpc.upsert).mock.results.at(-1)!.value;

    rpc.asyncInvokeAndAwait.mockReturnValueOnce({
      metadata: hostInvocationMetadata(),
      future: { get: vi.fn().mockResolvedValue(undefined), cancel: vi.fn() },
    });
    await expect(
      client.method('echo').invokeValue(v.record([v.string('input')])),
    ).rejects.toBeInstanceOf(RemoteOutputError);

    rpc.asyncInvokeAndAwait.mockReturnValueOnce({
      metadata: hostInvocationMetadata(),
      future: { get: vi.fn().mockResolvedValue(schemaValueToWit(v.u32(1))), cancel: vi.fn() },
    });
    await expect(
      client.method('echo').invokeValue(v.record([v.string('input')])),
    ).rejects.toBeInstanceOf(RemoteOutputError);
  });

  it('rejects malformed schema-native inputs before opening an RPC call', () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    const client = getAgentType('ReflectedEcho')!.client.upsert({ id: 'one' });
    const rpc = vi.mocked(WasmRpc.upsert).mock.results.at(-1)!.value;
    const method = client.method('echo');
    expect(() => method.triggerValue(v.record([]))).toThrow(/Invalid input/);
    expect(() => method.scheduleValue({ seconds: 1n, nanoseconds: 0 }, v.record([]))).toThrow(
      /Invalid input/,
    );
    expect(rpc.invoke).not.toHaveBeenCalled();
    expect(rpc.scheduleCancelableInvocation).not.toHaveBeenCalled();
  });

  it('rejects an unexpected value for a reflected unit output', async () => {
    const registered = registeredType();
    registered.agentType.methods[0].outputSchema = { tag: 'unit' };
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registered);
    const client = getAgentType('ReflectedEcho')!.client.upsert({ id: 'one' });
    const rpc = vi.mocked(WasmRpc.upsert).mock.results.at(-1)!.value;
    rpc.asyncInvokeAndAwait.mockReturnValueOnce({
      metadata: hostInvocationMetadata(),
      future: {
        get: vi.fn().mockResolvedValue(schemaValueToWit(v.string('unexpected'))),
        cancel: vi.fn(),
      },
    });
    await expect(
      client.method('echo').invokeValue(v.record([v.string('input')])),
    ).rejects.toBeInstanceOf(RemoteOutputError);
  });

  it('looks up the current schema for a concrete agent instance', () => {
    vi.mocked(hostGetAgentTypeByAgentId).mockReturnValueOnce(registeredType());

    expect(getAgentTypeByAgentId(ECHO_AGENT_ID)?.name).toBe('ReflectedEcho');
    expect(hostGetAgentTypeByAgentId).toHaveBeenLastCalledWith(ECHO_AGENT_ID);
  });

  it('uses discovery-packed values dynamically without inheriting reflected validation', async () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    const reflected = getAgentType('ReflectedEcho')!;
    const method = reflected.method('echo')!;
    const discoveredInput = method.input.packJson({ message: 'hello' });
    expect(method.input.validateValue(discoveredInput).success).toBe(true);

    vi.mocked(hostGetAgentTypeByAgentId).mockReturnValueOnce(registeredType());
    const before = vi.mocked(hostGetAgentType).mock.calls.length;
    const client = DynamicAgentClient.getById(ECHO_AGENT_ID);
    expect(vi.mocked(WasmRpc.getById).mock.calls.at(-1)).toEqual(['ReflectedEcho', ECHO_AGENT_ID]);
    const rpc = vi.mocked(WasmRpc.getById).mock.results.at(-1)!.value;
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: hostInvocationMetadata(),
      future: {
        get: vi.fn().mockResolvedValue(schemaValueToWit(v.u32(7))),
        cancel: vi.fn(),
      },
    });

    const result = await client.method(method.name).invokeValue(discoveredInput);
    expect(result).toMatchObject({
      value: v.u32(7),
    });
    expect(method.output!.validateValue(result.value!).success).toBe(false);

    await expect(client.method(method.name).invokeValue(v.record([]))).resolves.toBeDefined();
    expect(rpc.asyncInvokeAndAwait).toHaveBeenCalledTimes(2);
    expect(hostGetAgentType).toHaveBeenCalledTimes(before);
  });

  it('connects a discovered reflected type to an existing agent by ID', async () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    const reflected = getAgentType('ReflectedEcho')!;

    const client = reflected.client.getById(ECHO_AGENT_ID);

    expect(vi.mocked(WasmRpc.getById).mock.calls.at(-1)).toEqual(['ReflectedEcho', ECHO_AGENT_ID]);
    expect(client.method('echo').definition.name).toBe('echo');
  });

  it('throws on getById of an unknown or wrong-type ID with a typed not-found error', () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    const reflected = getAgentType('ReflectedEcho')!;
    vi.mocked(WasmRpc.getById).mockImplementationOnce(() => {
      throw { tag: 'not-found', val: 'no ReflectedEcho has this ID' };
    });

    let error: unknown;
    try {
      reflected.client.getById(ECHO_AGENT_ID);
    } catch (thrown) {
      error = thrown;
    }
    expect(error).toBeInstanceOf(RemoteCallError);
    expect(error).toMatchObject({
      cause: { tag: 'not-found', details: 'no ReflectedEcho has this ID' },
    });
  });

  it('reads the agent ID of a reflected client from the host', async () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    const client = getAgentType('ReflectedEcho')!.client.upsert({ id: 'one' });
    const rpc = vi.mocked(WasmRpc.upsert).mock.results.at(-1)!.value;

    await expect(client.agentId()).resolves.toEqual(AgentId.from(MOCK_REMOTE_AGENT_ID));
    expect(rpc.agentId).toHaveBeenCalledTimes(1);
  });

  it('maps a reflected create without keyed to a new unkeyed agent', async () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    await getAgentType('ReflectedEcho')!.client.create({ id: 'one' });

    const call = vi.mocked(WasmRpc.create).mock.calls.at(-1)!;
    expect(call[0]).toBe('ReflectedEcho');
    expect(call[1]).toEqual(schemaValueToWit(v.record([v.string('one')])));
    expect(call[2]).toEqual({ keyed: false, agentConfig: [], env: [] });
  });

  it('passes keyed, config and env of a reflected create to the host', async () => {
    const registered = registeredType();
    registered.agentType.config = [
      { source: 'local', path: ['greeting'], valueType: registered.agentType.schema.root },
    ];
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registered);
    const reflected = getAgentType('ReflectedEcho')!;

    await reflected.client.create(
      { id: 'one' },
      {
        keyed: true,
        config: [{ path: ['greeting'], value: 'hello' }],
        env: { MODE: 'test' },
      },
    );
    expect(vi.mocked(WasmRpc.create).mock.calls.at(-1)![2]).toEqual({
      keyed: true,
      agentConfig: [
        {
          path: ['greeting'],
          value: {
            graph: schemaGraphToWit(stringGraph),
            value: schemaValueToWit(v.string('hello')),
          },
        },
      ],
      env: [['MODE', 'test']],
    });

    await reflected.client.createValue(v.record([v.string('two')]), {
      keyed: true,
      config: [{ path: ['greeting'], value: { graph: stringGraph, value: v.string('hi') } }],
    });
    expect(vi.mocked(WasmRpc.create).mock.calls.at(-1)![2]).toMatchObject({
      keyed: true,
      agentConfig: [{ path: ['greeting'], value: { value: schemaValueToWit(v.string('hi')) } }],
      env: [],
    });
  });

  it('rejects a keyed reflected create of a current key with a typed conflict error', async () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    vi.mocked(WasmRpc.create).mockRejectedValueOnce({
      tag: 'conflict',
      val: 'ReflectedEcho("one") already exists',
    });

    const rejection = getAgentType('ReflectedEcho')!.client.create({ id: 'one' }, { keyed: true });
    await expect(rejection).rejects.toBeInstanceOf(RemoteCallError);
    await expect(rejection).rejects.toMatchObject({
      cause: { tag: 'conflict', details: 'ReflectedEcho("one") already exists' },
    });
  });

  it('validates reflected config before creating clients in both JSON and value styles', async () => {
    const registered = registeredType();
    registered.agentType.config = [
      { source: 'local', path: ['greeting'], valueType: registered.agentType.schema.root },
      { source: 'secret', path: ['apiKey'], valueType: registered.agentType.schema.root },
    ];
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registered);
    const reflected = getAgentType('ReflectedEcho')!;
    const creates = vi.mocked(WasmRpc.create).mock.calls.length;

    await expect(
      reflected.client.create({ id: 'one' }, { config: [{ path: ['other'], value: 'x' }] }),
    ).rejects.toThrow("Unknown config path 'other'");
    await expect(
      reflected.client.create({ id: 'one' }, { config: [{ path: ['apiKey'], value: 'x' }] }),
    ).rejects.toThrow("Cannot override secret config field 'apiKey'");
    await expect(
      reflected.client.create({ id: 'one' }, { config: [{ path: ['greeting'], value: 42 }] }),
    ).rejects.toThrow();
    await expect(
      reflected.client.createValue(v.record([v.string('one')]), {
        config: [{ path: ['greeting'], value: { graph: stringGraph, value: v.u32(42) } }],
      }),
    ).rejects.toThrow("Invalid config value at 'greeting'");
    await expect(
      reflected.client.createValue(v.record([v.string('one')]), {
        config: [
          {
            path: ['greeting'],
            value: { graph: { defs: new Map(), root: t.u32() }, value: v.string('hello') },
          },
        ],
      }),
    ).rejects.toThrow("Incompatible config schema at 'greeting'");
    expect(vi.mocked(WasmRpc.create)).toHaveBeenCalledTimes(creates);

    await expect(
      reflected.client.createValue(v.record([v.string('one')]), {
        config: [
          {
            path: ['greeting'],
            value: {
              graph: {
                defs: new Map([['alias', { name: 'Alias', body: t.string() }]]),
                root: t.ref('alias'),
              },
              value: v.string('hello'),
            },
          },
        ],
      }),
    ).resolves.toBeDefined();
    expect(vi.mocked(WasmRpc.create)).toHaveBeenCalledTimes(creates + 1);
  });

  it('rejects create and getById of a reflected ephemeral type', async () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType('ephemeral'));
    const reflected = getAgentType('ReflectedEcho')!;
    const creates = vi.mocked(WasmRpc.create).mock.calls.length;
    const lookups = vi.mocked(WasmRpc.getById).mock.calls.length;

    await expect(reflected.client.create({ id: 'one' })).rejects.toThrow(
      'create is not available for ephemeral agent types',
    );
    expect(() => reflected.client.getById(ECHO_AGENT_ID)).toThrow(
      'getById is not available for ephemeral agent types',
    );
    expect(WasmRpc.create).toHaveBeenCalledTimes(creates);
    expect(WasmRpc.getById).toHaveBeenCalledTimes(lookups);
  });

  it('preserves the structured RPC error returned by client upsert', () => {
    const rpcError = { tag: 'not-found' as const, val: 'missing deployment' };
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType());
    vi.mocked(WasmRpc.upsert).mockImplementationOnce(() => {
      throw rpcError;
    });

    try {
      getAgentType('ReflectedEcho')!.client.upsert({ id: 'one' });
      throw new Error('expected client upsert to fail');
    } catch (error) {
      expect(error).toBeInstanceOf(RemoteCallError);
      expect(error).toMatchObject({
        cause: { tag: 'not-found', details: rpcError.val },
      });
    }
  });

  it('returns invocation metadata and no agent ID for ephemeral reflected clients', async () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType('ephemeral'));
    const reflected = getAgentType('ReflectedEcho')!;
    const client = reflected.client.upsert({ id: 'one' });
    const rpc = vi.mocked(WasmRpc.upsert).mock.results.at(-1)!.value;
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: hostInvocationMetadata(),
      future: {
        get: vi.fn().mockResolvedValue(schemaValueToWit(v.string('hello'))),
        cancel: vi.fn(),
      },
    });

    expect(() => client.agentId()).toThrow('agentId is not available for ephemeral agent types');
    expect(rpc.agentId).not.toHaveBeenCalled();
    await expect(client.method('echo').invoke({ message: 'hello' })).resolves.toEqual({
      metadata: sdkInvocationMetadata(),
      value: 'hello',
    });
  });
});
