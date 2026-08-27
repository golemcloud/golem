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
import { dynamicClient, getAgentType, getAgentTypeByAgentId } from '../src/reflection';
import { RemoteCallError } from '../src/client';
import { Uuid } from '../src/uuid';

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
});

describe('agent reflection', () => {
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

  it('looks up the current schema for a concrete agent instance', () => {
    const rawId = {
      componentId: { uuid: { highBits: 0n, lowBits: 1n } },
      agentId: 'ReflectedEcho(one)',
    };
    vi.mocked(hostGetAgentTypeByAgentId).mockReturnValueOnce(registeredType());

    expect(getAgentTypeByAgentId(rawId)?.name).toBe('ReflectedEcho');
    expect(hostGetAgentTypeByAgentId).toHaveBeenLastCalledWith(rawId);
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
    const client = dynamicClient({
      componentId: { uuid: { highBits: 0n, lowBits: 1n } },
      agentId: 'ReflectedEcho(one)',
    });
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
      expect(error).toMatchObject({ rpcError, cause: rpcError });
    }
  });

  it('supports known and new phantom clients for ephemeral reflected types', () => {
    vi.mocked(hostGetAgentType).mockReturnValueOnce(registeredType('ephemeral'));
    const reflected = getAgentType('ReflectedEcho')!;
    const phantomId = new Uuid(1n, 2n);
    const known = reflected.client.getPhantom({ id: 'one' }, phantomId);
    const fresh = reflected.client.newPhantom({ id: 'two' });

    expect(reflected.agentId({ id: 'one' }, phantomId)).toMatchObject({
      agentId: 'MockAgent()',
      componentId: reflected.implementedBy,
    });
    expect(known.agentId).toEqual(reflected.agentId({ id: 'one' }, phantomId));
    expect(fresh).toBeInstanceOf(Object);
    expect('client' in fresh).toBe(false);
  });
});
