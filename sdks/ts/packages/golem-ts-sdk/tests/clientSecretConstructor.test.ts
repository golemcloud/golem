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

import { afterEach, describe, expect, it, vi } from 'vitest';

describe('remote client owned secret constructor inputs', () => {
  afterEach(() => {
    vi.doUnmock('golem:agent/host@2.0.0');
    vi.doUnmock('golem:secrets/reveal@0.1.0');
    vi.resetModules();
  });

  it('review repro: rejects constructor secrets before makeAgentId can consume them', async () => {
    let wasmRpcConstructorTree: { valueNodes: { tag: string; val?: unknown }[] } | undefined;
    let makeAgentIdCalled = false;

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({
      makeAgentId: (
        _agentTypeName: string,
        input: { valueNodes: { tag: string; val?: unknown }[] },
      ) => {
        makeAgentIdCalled = true;
        for (const node of input.valueNodes) {
          if (node.tag === 'secret-value') {
            node.val = undefined;
          }
        }
        return 'SecretCtorAgent(mocked)';
      },
      parseAgentId: () => {
        throw new Error('parseAgentId should not be called by this test');
      },
      getConfigValue: () => {
        throw new Error('getConfigValue should not be called by this test');
      },
      Datetime: class {},
      WasmRpc: vi
        .fn()
        .mockImplementation(
          (
            _agentTypeName: string,
            constructorTree: { valueNodes: { tag: string; val?: unknown }[] },
          ) => {
            wasmRpcConstructorTree = constructorTree;
            return {
              invokeAndAwait: vi.fn(),
              invoke: vi.fn(),
              asyncInvokeAndAwait: vi.fn(),
              scheduleInvocation: vi.fn(),
              scheduleCancelableInvocation: vi.fn(),
            };
          },
        ),
    }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal: vi.fn() }));

    const { TypeMetadata } = await import('@golemcloud/golem-ts-types-core');
    const { getRemoteClient } = await import('../src/internal/clientGeneration');
    const { AgentConstructorParamRegistry } =
      await import('../src/internal/registry/agentConstructorParamRegistry');
    const { AgentClassName } = await import('../src/agentClassName');
    const { r } = await import('../src/internal/mapping/types/resolvedType');
    const { Secret } = await import('../src/agentConfig');
    const { GuestSecretHandle } = await import('../src/internal/schema-model/secretHandle');
    const { SECRET_INTERNAL } = await import('../src/internal/schema-model/secretInternal');

    class SecretCtorAgent {}

    const stringType = { kind: 'string' as const, optional: false };
    const secretType = {
      kind: 'secret' as const,
      optional: false,
      element: stringType,
    };
    TypeMetadata.update('SecretCtorAgent', [{ name: 'secret', type: secretType }], new Map());
    AgentConstructorParamRegistry.setType('SecretCtorAgent', 'secret', {
      tag: 'schema',
      graph: { defs: new Map(), root: r.secret(r.string()) },
      tsType: secretType,
    });

    const rawSecret = { id: 'opaque-secret' } as never;
    const handle = GuestSecretHandle.fromRaw(SECRET_INTERNAL, rawSecret);
    const secret = Secret._fromHandle<string>(SECRET_INTERNAL, handle, {
      defs: new Map(),
      root: r.string(),
    });
    const getClient = getRemoteClient(
      new AgentClassName('SecretCtorAgent'),
      { typeName: 'SecretCtorAgent' } as never,
      SecretCtorAgent,
      false,
    );

    expect(() => getClient(secret)).toThrow(/cannot contain Secret<T>/);

    expect(makeAgentIdCalled).toBe(false);
    expect(wasmRpcConstructorTree).toBeUndefined();
    expect(handle.isPresent()).toBe(true);
  });

  it('review repro: omitted optional constructor secrets do not block stable agent id creation', async () => {
    let wasmRpcConstructorTree: { valueNodes: { tag: string; val?: unknown }[] } | undefined;
    let makeAgentIdCalled = false;

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({
      makeAgentId: (
        _agentTypeName: string,
        input: { valueNodes: { tag: string; val?: unknown }[] },
      ) => {
        makeAgentIdCalled = true;
        return `OptionalSecretCtorAgent(${JSON.stringify(input)})`;
      },
      parseAgentId: () => {
        throw new Error('parseAgentId should not be called by this test');
      },
      getConfigValue: () => {
        throw new Error('getConfigValue should not be called by this test');
      },
      Datetime: class {},
      WasmRpc: vi
        .fn()
        .mockImplementation(
          (
            _agentTypeName: string,
            constructorTree: { valueNodes: { tag: string; val?: unknown }[] },
          ) => {
            wasmRpcConstructorTree = constructorTree;
            return {
              invokeAndAwait: vi.fn(),
              invoke: vi.fn(),
              asyncInvokeAndAwait: vi.fn(),
              scheduleInvocation: vi.fn(),
              scheduleCancelableInvocation: vi.fn(),
            };
          },
        ),
    }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal: vi.fn() }));

    const { TypeMetadata } = await import('@golemcloud/golem-ts-types-core');
    const { getRemoteClient } = await import('../src/internal/clientGeneration');
    const { AgentConstructorParamRegistry } =
      await import('../src/internal/registry/agentConstructorParamRegistry');
    const { AgentClassName } = await import('../src/agentClassName');
    const { r } = await import('../src/internal/mapping/types/resolvedType');

    class OptionalSecretCtorAgent {}

    const stringType = { kind: 'string' as const, optional: false };
    const optionalSecretType = {
      kind: 'secret' as const,
      optional: true,
      element: stringType,
    };
    TypeMetadata.update(
      'OptionalSecretCtorAgent',
      [{ name: 'secret', type: optionalSecretType }],
      new Map(),
    );
    AgentConstructorParamRegistry.setType('OptionalSecretCtorAgent', 'secret', {
      tag: 'schema',
      graph: { defs: new Map(), root: r.option(r.secret(r.string()), 'undefined') },
      tsType: optionalSecretType,
    });

    const getClient = getRemoteClient(
      new AgentClassName('OptionalSecretCtorAgent'),
      { typeName: 'OptionalSecretCtorAgent' } as never,
      OptionalSecretCtorAgent,
      false,
    );

    expect(() => getClient()).not.toThrow();
    expect(makeAgentIdCalled).toBe(true);
    expect(wasmRpcConstructorTree).toBeDefined();
  });

  it('review repro: rejects constructor secrets hidden inside multimodal parameters before encoding consumes them', async () => {
    let wasmRpcConstructorTree: { valueNodes: { tag: string; val?: unknown }[] } | undefined;
    let makeAgentIdCalled = false;

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({
      makeAgentId: (
        _agentTypeName: string,
        input: { valueNodes: { tag: string; val?: unknown }[] },
      ) => {
        makeAgentIdCalled = true;
        for (const node of input.valueNodes) {
          if (node.tag === 'secret-value') {
            node.val = undefined;
          }
        }
        return 'SecretMultimodalCtorAgent(mocked)';
      },
      parseAgentId: () => {
        throw new Error('parseAgentId should not be called by this test');
      },
      getConfigValue: () => {
        throw new Error('getConfigValue should not be called by this test');
      },
      Datetime: class {},
      WasmRpc: vi
        .fn()
        .mockImplementation(
          (
            _agentTypeName: string,
            constructorTree: { valueNodes: { tag: string; val?: unknown }[] },
          ) => {
            wasmRpcConstructorTree = constructorTree;
            return {
              invokeAndAwait: vi.fn(),
              invoke: vi.fn(),
              asyncInvokeAndAwait: vi.fn(),
              scheduleInvocation: vi.fn(),
              scheduleCancelableInvocation: vi.fn(),
            };
          },
        ),
    }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal: vi.fn() }));

    const { TypeMetadata } = await import('@golemcloud/golem-ts-types-core');
    const { getRemoteClient } = await import('../src/internal/clientGeneration');
    const { AgentConstructorParamRegistry } =
      await import('../src/internal/registry/agentConstructorParamRegistry');
    const { AgentClassName } = await import('../src/agentClassName');
    const { r } = await import('../src/internal/mapping/types/resolvedType');
    const { Secret } = await import('../src/agentConfig');
    const { GuestSecretHandle } = await import('../src/internal/schema-model/secretHandle');
    const { SECRET_INTERNAL } = await import('../src/internal/schema-model/secretInternal');

    class SecretMultimodalCtorAgent {}

    const stringType = { kind: 'string' as const, optional: false };
    const secretType = {
      kind: 'secret' as const,
      optional: false,
      element: stringType,
    };
    const multimodalType = {
      kind: 'array' as const,
      optional: false,
      element: { kind: 'object' as const, optional: false, properties: [] },
    };
    TypeMetadata.update(
      'SecretMultimodalCtorAgent',
      [{ name: 'input', type: multimodalType }],
      new Map(),
    );
    AgentConstructorParamRegistry.setType('SecretMultimodalCtorAgent', 'input', {
      tag: 'multimodal',
      cases: [
        {
          name: 'secret',
          type: {
            tag: 'schema',
            graph: { defs: new Map(), root: r.secret(r.string()) },
            tsType: secretType,
          },
        },
      ],
      tsType: multimodalType,
    });

    const rawSecret = { id: 'opaque-secret' } as never;
    const handle = GuestSecretHandle.fromRaw(SECRET_INTERNAL, rawSecret);
    const secret = Secret._fromHandle<string>(SECRET_INTERNAL, handle, {
      defs: new Map(),
      root: r.string(),
    });
    const getClient = getRemoteClient(
      new AgentClassName('SecretMultimodalCtorAgent'),
      { typeName: 'SecretMultimodalCtorAgent' } as never,
      SecretMultimodalCtorAgent,
      false,
    );

    let thrown: unknown;
    try {
      getClient([{ tag: 'secret', val: secret }]);
    } catch (e) {
      thrown = e;
    }

    expect({
      threwSecretConstructorError:
        thrown instanceof Error && /cannot contain Secret<T>/.test(thrown.message),
      makeAgentIdCalled,
      wasmRpcConstructed: wasmRpcConstructorTree !== undefined,
      handlePresent: handle.isPresent(),
    }).toEqual({
      threwSecretConstructorError: true,
      makeAgentIdCalled: false,
      wasmRpcConstructed: false,
      handlePresent: true,
    });
  });

  it('review repro: remote config overrides preserve explicit null option values', async () => {
    let capturedConfigEntries: { path: string[] }[] | undefined;

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({
      makeAgentId: () => 'NullableConfigAgent(mocked)',
      parseAgentId: () => {
        throw new Error('parseAgentId should not be called by this test');
      },
      getConfigValue: () => {
        throw new Error('getConfigValue should not be called by this test');
      },
      Datetime: class {},
      WasmRpc: vi
        .fn()
        .mockImplementation(
          (
            _agentTypeName: string,
            _constructorTree: unknown,
            _phantomId: unknown,
            agentConfigEntries: { path: string[] }[],
          ) => {
            capturedConfigEntries = agentConfigEntries;
            return {
              invokeAndAwait: vi.fn(),
              invoke: vi.fn(),
              asyncInvokeAndAwait: vi.fn(),
              scheduleInvocation: vi.fn(),
              scheduleCancelableInvocation: vi.fn(),
            };
          },
        ),
    }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal: vi.fn() }));

    const { TypeMetadata, buildTypeFromJSON } = await import('@golemcloud/golem-ts-types-core');
    const { getRemoteClient } = await import('../src/internal/clientGeneration');
    const { AgentConstructorParamRegistry } =
      await import('../src/internal/registry/agentConstructorParamRegistry');
    const { AgentClassName } = await import('../src/agentClassName');

    class NullableConfigAgent {}

    const maybeLabelType = buildTypeFromJSON({
      kind: 'union',
      optional: false,
      types: [
        { kind: 'string', optional: false },
        { kind: 'null', optional: false },
      ],
      typeParams: [],
      originalTypeName: undefined,
    });
    const configType = {
      kind: 'config' as const,
      optional: false,
      properties: [
        {
          path: ['maybeLabel'],
          secret: false,
          type: maybeLabelType,
        },
      ],
      requiredMembers: [],
    };

    TypeMetadata.update('NullableConfigAgent', [{ name: 'config', type: configType }], new Map());
    AgentConstructorParamRegistry.setType('NullableConfigAgent', 'config', {
      tag: 'config',
      tsType: configType,
    });

    const getClient = getRemoteClient(
      new AgentClassName('NullableConfigAgent'),
      { typeName: 'NullableConfigAgent' } as never,
      NullableConfigAgent,
      true,
    );

    getClient({ maybeLabel: null });

    expect(capturedConfigEntries?.map((entry) => entry.path)).toEqual([['maybeLabel']]);
  });
});
