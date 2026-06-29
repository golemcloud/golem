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
import { Secret } from '../src/agentConfig';
import { getRemoteClient } from '../src/internal/clientGeneration';
import { AgentConstructorParamRegistry } from '../src/internal/registry/agentConstructorParamRegistry';
import { AgentClassName } from '../src/agentClassName';
import { r } from '../src/internal/mapping/types/resolvedType';
import { GuestSecretHandle } from '../src/internal/schema-model/secretHandle';
import { SECRET_INTERNAL } from '../src/internal/schema-model/secretInternal';

describe('opaque secret config override runtime rejection edge cases', () => {
  it('rejects non-enumerable secrets nested inside non-secret config override leaves', () => {
    vi.mocked(WasmRpc).mockClear();

    class NonEnumerableSecretConfigAgent {}

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
      'NonEnumerableSecretConfigAgent',
      [{ name: 'config', type: configType }],
      new Map(),
    );
    AgentConstructorParamRegistry.setType('NonEnumerableSecretConfigAgent', 'config', {
      tag: 'config',
      tsType: configType,
    });

    const rawSecret = { id: 'non-enumerable-config-secret' } as never;
    const handle = GuestSecretHandle.fromRaw(SECRET_INTERNAL, rawSecret);
    const secret = Secret._fromHandle<string>(SECRET_INTERNAL, handle, {
      defs: new Map(),
      root: r.string(),
    });
    const entry = { label: 'prod' } as { label: string; token: Secret<string> };
    Object.defineProperty(entry, 'token', {
      value: secret,
      enumerable: false,
      configurable: true,
    });

    const getClient = getRemoteClient(
      new AgentClassName('NonEnumerableSecretConfigAgent'),
      { typeName: 'NonEnumerableSecretConfigAgent' } as never,
      NonEnumerableSecretConfigAgent,
      true,
    );

    let thrown: unknown;
    try {
      getClient({ entries: [entry] } as any);
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
});
