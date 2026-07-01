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
import { WasmRpc } from 'golem:agent/host@2.0.0';
import { Node, Symbol, TypeMetadata } from '@golemcloud/golem-ts-types-core';
import { agent, BaseAgent } from '../src';
import { Secret } from '../src/agentConfig';
import { ConfigAgent } from './validAgents';
import { getRemoteClient } from '../src/internal/clientGeneration';
import { AgentConstructorParamRegistry } from '../src/internal/registry/agentConstructorParamRegistry';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { AgentClassName } from '../src/agentClassName';
import { r } from '../src/internal/mapping/types/resolvedType';
import { GuestSecretHandle } from '../src/internal/schema-model/secretHandle';
import { SECRET_INTERNAL } from '../src/internal/schema-model/secretInternal';

describe('opaque secret config override runtime rejection', () => {
  it('rejects a direct secret field supplied through getWithConfig at runtime', () => {
    vi.mocked(WasmRpc).mockClear();

    let thrown: unknown;
    try {
      ConfigAgent.getWithConfig(1, true, {
        secret: new Secret<boolean>(['secret'], { kind: 'boolean', optional: false }),
      } as any);
    } catch (e) {
      thrown = e;
    }

    expect({
      rejectedSecretOverride: thrown instanceof Error && /secret/i.test(thrown.message),
      wasmRpcConstructed: vi.mocked(WasmRpc).mock.calls.length > 0,
    }).toEqual({
      rejectedSecretOverride: true,
      wasmRpcConstructed: false,
    });
  });

  it('rejects an explicit undefined secret field supplied through getWithConfig at runtime', () => {
    vi.mocked(WasmRpc).mockClear();

    let thrown: unknown;
    try {
      ConfigAgent.getWithConfig(1, true, {
        secret: undefined,
      } as any);
    } catch (e) {
      thrown = e;
    }

    expect({
      rejectedSecretOverride: thrown instanceof Error && /secret/i.test(thrown.message),
      wasmRpcConstructed: vi.mocked(WasmRpc).mock.calls.length > 0,
    }).toEqual({
      rejectedSecretOverride: true,
      wasmRpcConstructed: false,
    });
  });

  it('rejects a secret nested inside a non-secret config override leaf at runtime', () => {
    vi.mocked(WasmRpc).mockClear();

    class RuntimeNestedSecretConfigAgent {}

    const stringType = { kind: 'string' as const, optional: false };
    const secretType = {
      kind: 'secret' as const,
      optional: false,
      element: stringType,
    };
    const entryType = {
      kind: 'object' as const,
      optional: false,
      properties: [
        new Symbol({
          name: 'label',
          declarations: [new Node('PropertySignature')],
          typeAtLocation: stringType,
        }),
        new Symbol({
          name: 'token',
          declarations: [new Node('PropertySignature')],
          typeAtLocation: secretType,
        }),
      ],
      typeParams: [],
    };
    const configType = {
      kind: 'config' as const,
      optional: false,
      properties: [
        {
          path: ['entries'],
          secret: false,
          type: { kind: 'array' as const, optional: false, element: entryType },
        },
      ],
      requiredMembers: [],
    };

    TypeMetadata.update(
      'RuntimeNestedSecretConfigAgent',
      [{ name: 'config', type: configType }],
      new Map(),
    );
    AgentConstructorParamRegistry.setType('RuntimeNestedSecretConfigAgent', 'config', {
      tag: 'config',
      tsType: configType,
    });

    const rawSecret = { id: 'nested-config-secret' } as never;
    const handle = GuestSecretHandle.fromRaw(SECRET_INTERNAL, rawSecret);
    const secret = Secret._fromHandle<string>(SECRET_INTERNAL, handle, {
      defs: new Map(),
      root: r.string(),
    });
    const getClient = getRemoteClient(
      new AgentClassName('RuntimeNestedSecretConfigAgent'),
      { typeName: 'RuntimeNestedSecretConfigAgent' } as never,
      RuntimeNestedSecretConfigAgent,
      true,
    );

    let thrown: unknown;
    try {
      getClient({
        entries: [{ label: 'prod', token: secret }],
      } as any);
    } catch (e) {
      thrown = e;
    }

    expect({
      rejectedSecretOverride: thrown instanceof Error && /secret/i.test(thrown.message),
      wasmRpcConstructed: vi.mocked(WasmRpc).mock.calls.length > 0,
      handleStillPresent: handle.isPresent(),
    }).toEqual({
      rejectedSecretOverride: true,
      wasmRpcConstructed: false,
      handleStillPresent: true,
    });
  });

  it('rejects explicit undefined for an optional secret nested inside a config override array', () => {
    vi.mocked(WasmRpc).mockClear();

    class RuntimeNestedUndefinedSecretConfigAgent {}

    const stringType = { kind: 'string' as const, optional: false };
    const secretType = {
      kind: 'secret' as const,
      optional: false,
      element: stringType,
    };
    const entryType = {
      kind: 'object' as const,
      optional: false,
      properties: [
        new Symbol({
          name: 'label',
          declarations: [new Node('PropertySignature')],
          typeAtLocation: stringType,
        }),
        new Symbol({
          name: 'token',
          declarations: [new Node('PropertySignature', true)],
          typeAtLocation: secretType,
        }),
      ],
      typeParams: [],
    };
    const configType = {
      kind: 'config' as const,
      optional: false,
      properties: [
        {
          path: ['entries'],
          secret: false,
          type: { kind: 'array' as const, optional: false, element: entryType },
        },
      ],
      requiredMembers: [],
    };

    TypeMetadata.update(
      'RuntimeNestedUndefinedSecretConfigAgent',
      [{ name: 'config', type: configType }],
      new Map(),
    );
    AgentConstructorParamRegistry.setType('RuntimeNestedUndefinedSecretConfigAgent', 'config', {
      tag: 'config',
      tsType: configType,
    });

    const getClient = getRemoteClient(
      new AgentClassName('RuntimeNestedUndefinedSecretConfigAgent'),
      { typeName: 'RuntimeNestedUndefinedSecretConfigAgent' } as never,
      RuntimeNestedUndefinedSecretConfigAgent,
      true,
    );

    let thrown: unknown;
    try {
      getClient({
        entries: [{ label: 'prod', token: undefined }],
      } as any);
    } catch (e) {
      thrown = e;
    }

    expect({
      rejectedSecretOverride: thrown instanceof Error && /secret/i.test(thrown.message),
      wasmRpcConstructed: vi.mocked(WasmRpc).mock.calls.length > 0,
    }).toEqual({
      rejectedSecretOverride: true,
      wasmRpcConstructed: false,
    });
  });

  it('rejects non-secret config override leaves whose union type can carry secrets', () => {
    vi.mocked(WasmRpc).mockClear();

    class RuntimeUnionSecretConfigAgent {}

    const stringType = { kind: 'string' as const, optional: false };
    const secretType = {
      kind: 'secret' as const,
      optional: false,
      element: stringType,
    };
    const configType = {
      kind: 'config' as const,
      optional: false,
      properties: [
        {
          path: ['credential'],
          secret: false,
          type: {
            kind: 'union' as const,
            optional: false,
            unionTypes: [stringType, secretType],
            typeParams: [],
            originalTypeName: undefined,
          },
        },
      ],
      requiredMembers: [],
    };

    TypeMetadata.update(
      'RuntimeUnionSecretConfigAgent',
      [{ name: 'config', type: configType }],
      new Map(),
    );
    AgentConstructorParamRegistry.setType('RuntimeUnionSecretConfigAgent', 'config', {
      tag: 'config',
      tsType: configType,
    });

    const getClient = getRemoteClient(
      new AgentClassName('RuntimeUnionSecretConfigAgent'),
      { typeName: 'RuntimeUnionSecretConfigAgent' } as never,
      RuntimeUnionSecretConfigAgent,
      true,
    );

    let thrown: unknown;
    try {
      getClient({ credential: 'public-fallback' } as any);
    } catch (e) {
      thrown = e;
    }

    expect({
      rejectedSecretOverride: thrown instanceof Error && /secret/i.test(thrown.message),
      wasmRpcConstructed: vi.mocked(WasmRpc).mock.calls.length > 0,
    }).toEqual({
      rejectedSecretOverride: true,
      wasmRpcConstructed: false,
    });
  });

  it('rejects explicit undefined for a non-secret config leaf whose union type can carry secrets', () => {
    vi.mocked(WasmRpc).mockClear();

    class RuntimeUndefinedUnionSecretConfigAgent {}

    const stringType = { kind: 'string' as const, optional: false };
    const secretType = {
      kind: 'secret' as const,
      optional: false,
      element: stringType,
    };
    const configType = {
      kind: 'config' as const,
      optional: false,
      properties: [
        {
          path: ['credential'],
          secret: false,
          type: {
            kind: 'union' as const,
            optional: false,
            unionTypes: [stringType, secretType],
            typeParams: [],
            originalTypeName: undefined,
          },
        },
      ],
      requiredMembers: [],
    };

    TypeMetadata.update(
      'RuntimeUndefinedUnionSecretConfigAgent',
      [{ name: 'config', type: configType }],
      new Map(),
    );
    AgentConstructorParamRegistry.setType('RuntimeUndefinedUnionSecretConfigAgent', 'config', {
      tag: 'config',
      tsType: configType,
    });

    const getClient = getRemoteClient(
      new AgentClassName('RuntimeUndefinedUnionSecretConfigAgent'),
      { typeName: 'RuntimeUndefinedUnionSecretConfigAgent' } as never,
      RuntimeUndefinedUnionSecretConfigAgent,
      true,
    );

    let thrown: unknown;
    try {
      getClient({ credential: undefined } as any);
    } catch (e) {
      thrown = e;
    }

    expect({
      rejectedSecretOverride: thrown instanceof Error && /secret/i.test(thrown.message),
      wasmRpcConstructed: vi.mocked(WasmRpc).mock.calls.length > 0,
    }).toEqual({
      rejectedSecretOverride: true,
      wasmRpcConstructed: false,
    });
  });

  it('allows overriding a non-secret sibling without touching a secret in the same group', () => {
    vi.mocked(WasmRpc).mockClear();

    class RuntimeSecretSiblingConfigAgent {}

    const stringType = { kind: 'string' as const, optional: false };
    const numberType = { kind: 'number' as const, optional: false };
    const configType = {
      kind: 'config' as const,
      optional: false,
      properties: [
        {
          path: ['auth', 'apiKey'],
          secret: true,
          type: stringType,
        },
        {
          path: ['auth', 'timeoutSeconds'],
          secret: false,
          type: numberType,
        },
      ],
      requiredMembers: [],
    };

    TypeMetadata.update(
      'RuntimeSecretSiblingConfigAgent',
      [{ name: 'config', type: configType }],
      new Map(),
    );
    AgentConstructorParamRegistry.setType('RuntimeSecretSiblingConfigAgent', 'config', {
      tag: 'config',
      tsType: configType,
    });

    const getClient = getRemoteClient(
      new AgentClassName('RuntimeSecretSiblingConfigAgent'),
      { typeName: 'RuntimeSecretSiblingConfigAgent' } as never,
      RuntimeSecretSiblingConfigAgent,
      true,
    );

    let thrown: unknown;
    try {
      getClient({ auth: { timeoutSeconds: 30 } } as any);
    } catch (e) {
      thrown = e;
    }

    expect({
      threw: thrown instanceof Error ? thrown.message : undefined,
      wasmRpcConstructed: vi.mocked(WasmRpc).mock.calls.length > 0,
    }).toEqual({
      threw: undefined,
      wasmRpcConstructed: true,
    });
  });

  it('rejects an empty group whose only leaf is a non-direct secret-bearing config type', () => {
    vi.mocked(WasmRpc).mockClear();

    class RuntimeSecretArrayEmptyGroupConfigAgent {}

    const stringType = { kind: 'string' as const, optional: false };
    const secretType = {
      kind: 'secret' as const,
      optional: false,
      element: stringType,
    };
    const configType = {
      kind: 'config' as const,
      optional: false,
      properties: [
        {
          path: ['auth', 'tokens'],
          secret: false,
          type: { kind: 'array' as const, optional: false, element: secretType },
        },
      ],
      requiredMembers: [],
    };

    TypeMetadata.update(
      'RuntimeSecretArrayEmptyGroupConfigAgent',
      [{ name: 'config', type: configType }],
      new Map(),
    );
    AgentConstructorParamRegistry.setType('RuntimeSecretArrayEmptyGroupConfigAgent', 'config', {
      tag: 'config',
      tsType: configType,
    });

    const getClient = getRemoteClient(
      new AgentClassName('RuntimeSecretArrayEmptyGroupConfigAgent'),
      { typeName: 'RuntimeSecretArrayEmptyGroupConfigAgent' } as never,
      RuntimeSecretArrayEmptyGroupConfigAgent,
      true,
    );

    let thrown: unknown;
    try {
      getClient({ auth: {} } as any);
    } catch (e) {
      thrown = e;
    }

    expect({
      rejectedSecretOverride: thrown instanceof Error && /secret/i.test(thrown.message),
      wasmRpcConstructed: vi.mocked(WasmRpc).mock.calls.length > 0,
    }).toEqual({
      rejectedSecretOverride: true,
      wasmRpcConstructed: false,
    });
  });

  it('initiates an agent from WIT constructor input that carries a Secret<T>', () => {
    class InitiateFromWitSecretCtorAgent extends BaseAgent {
      constructor(readonly credential: Secret<string>) {
        super();
      }
    }

    TypeMetadata.update(
      'InitiateFromWitSecretCtorAgent',
      [
        {
          name: 'credential',
          type: {
            kind: 'secret',
            optional: false,
            element: { kind: 'string', optional: false },
          },
        },
      ],
      new Map(),
    );
    agent()(InitiateFromWitSecretCtorAgent);

    const initiator = AgentInitiatorRegistry.lookup('InitiateFromWitSecretCtorAgent');
    if (!initiator?.initiateFromWit) {
      throw new Error('InitiateFromWitSecretCtorAgent initiator was not registered');
    }

    (globalThis as any).currentAgentId = 'InitiateFromWitSecretCtorAgent(secret-constructor)';

    const rawSecret = { id: 'constructor-secret' } as never;
    const constructorInput = {
      valueNodes: [
        { tag: 'secret-value', val: rawSecret },
        { tag: 'record-value', val: [0] },
      ],
      root: 1,
    } as Parameters<typeof initiator.initiateFromWit>[0];

    let result: ReturnType<typeof initiator.initiateFromWit> | undefined;
    expect(() => {
      result = initiator.initiateFromWit!(constructorInput, { tag: 'anonymous' });
    }).not.toThrow();

    expect(result?.tag).toBe('ok');
    expect(result?.tag === 'ok' ? result.val.getParameters().tag : undefined).toBe('record');
  });
});
