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

describe('opaque secret config override adversarial repros', () => {
  afterEach(() => {
    vi.doUnmock('golem:agent/host@2.0.0');
    vi.doUnmock('golem:secrets/reveal@0.1.0');
    vi.resetModules();
  });

  it('rejects explicit undefined for a nested config group whose only leaf is a secret', async () => {
    let wasmRpcConstructed = false;

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({
      makeAgentId: () => 'NestedSecretConfigAgent(mocked)',
      parseAgentId: () => {
        throw new Error('parseAgentId should not be called by this test');
      },
      getConfigValue: () => {
        throw new Error('getConfigValue should not be called by this test');
      },
      Datetime: class {},
      WasmRpc: vi.fn().mockImplementation(() => {
        wasmRpcConstructed = true;
        return {
          invokeAndAwait: vi.fn(),
          invoke: vi.fn(),
          asyncInvokeAndAwait: vi.fn(),
          scheduleInvocation: vi.fn(),
          scheduleCancelableInvocation: vi.fn(),
        };
      }),
    }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal: vi.fn() }));

    const { TypeMetadata } = await import('@golemcloud/golem-ts-types-core');
    const { getRemoteClient } = await import('../src/internal/clientGeneration');
    const { AgentConstructorParamRegistry } =
      await import('../src/internal/registry/agentConstructorParamRegistry');
    const { AgentClassName } = await import('../src/agentClassName');

    class NestedSecretConfigAgent {}

    const stringType = { kind: 'string' as const, optional: false };
    const configType = {
      kind: 'config' as const,
      optional: false,
      properties: [
        {
          path: ['auth', 'apiKey'],
          secret: true,
          type: stringType,
        },
      ],
      requiredMembers: [
        { path: [], requiredKeys: ['auth'] },
        { path: ['auth'], requiredKeys: ['apiKey'] },
      ],
    };

    TypeMetadata.update(
      'NestedSecretConfigAgent',
      [{ name: 'config', type: configType }],
      new Map(),
    );
    AgentConstructorParamRegistry.setType('NestedSecretConfigAgent', 'config', {
      tag: 'config',
      tsType: configType,
    });

    const getClient = getRemoteClient(
      new AgentClassName('NestedSecretConfigAgent'),
      { typeName: 'NestedSecretConfigAgent' } as never,
      NestedSecretConfigAgent,
      true,
    );

    let thrown: unknown;
    try {
      getClient({ auth: undefined } as never);
    } catch (e) {
      thrown = e;
    }

    expect({
      rejectedSecretOverride: thrown instanceof Error && /secret/i.test(thrown.message),
      wasmRpcConstructed,
    }).toEqual({
      rejectedSecretOverride: true,
      wasmRpcConstructed: false,
    });
  });

  it('rejects an empty nested config group whose only leaf is a secret', async () => {
    let wasmRpcConstructed = false;

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({
      makeAgentId: () => 'NestedEmptySecretConfigAgent(mocked)',
      parseAgentId: () => {
        throw new Error('parseAgentId should not be called by this test');
      },
      getConfigValue: () => {
        throw new Error('getConfigValue should not be called by this test');
      },
      Datetime: class {},
      WasmRpc: vi.fn().mockImplementation(() => {
        wasmRpcConstructed = true;
        return {
          invokeAndAwait: vi.fn(),
          invoke: vi.fn(),
          asyncInvokeAndAwait: vi.fn(),
          scheduleInvocation: vi.fn(),
          scheduleCancelableInvocation: vi.fn(),
        };
      }),
    }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal: vi.fn() }));

    const { TypeMetadata } = await import('@golemcloud/golem-ts-types-core');
    const { getRemoteClient } = await import('../src/internal/clientGeneration');
    const { AgentConstructorParamRegistry } =
      await import('../src/internal/registry/agentConstructorParamRegistry');
    const { AgentClassName } = await import('../src/agentClassName');

    class NestedEmptySecretConfigAgent {}

    const stringType = { kind: 'string' as const, optional: false };
    const configType = {
      kind: 'config' as const,
      optional: false,
      properties: [
        {
          path: ['auth', 'apiKey'],
          secret: true,
          type: stringType,
        },
      ],
      requiredMembers: [
        { path: [], requiredKeys: ['auth'] },
        { path: ['auth'], requiredKeys: ['apiKey'] },
      ],
    };

    TypeMetadata.update(
      'NestedEmptySecretConfigAgent',
      [{ name: 'config', type: configType }],
      new Map(),
    );
    AgentConstructorParamRegistry.setType('NestedEmptySecretConfigAgent', 'config', {
      tag: 'config',
      tsType: configType,
    });

    const getClient = getRemoteClient(
      new AgentClassName('NestedEmptySecretConfigAgent'),
      { typeName: 'NestedEmptySecretConfigAgent' } as never,
      NestedEmptySecretConfigAgent,
      true,
    );

    let thrown: unknown;
    try {
      getClient({ auth: {} } as never);
    } catch (e) {
      thrown = e;
    }

    expect({
      rejectedSecretOverride: thrown instanceof Error && /secret/i.test(thrown.message),
      wasmRpcConstructed,
    }).toEqual({
      rejectedSecretOverride: true,
      wasmRpcConstructed: false,
    });
  });
});
