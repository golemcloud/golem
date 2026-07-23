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

import { describe, it, expect } from 'vitest';
import { compileSchema } from '../src/fluent/schema/adapter';
import { s } from '../src/fluent/schema/markers';
import { sdkPrincipalFromHost, Principal } from '../src/principal';

// `s.principal()` carries an SDK `Principal` as a WIT `principal` variant value,
// round-tripping SDK Principal <-> SchemaValue via the host shape.
describe('fluent s.principal() marker', () => {
  it('builds a variant graph with cases oidc/agent/golem-user/anonymous', () => {
    const codec = compileSchema(s.principal());
    const root = codec.graph.root.body as { tag: string; cases: { name: string }[] };
    expect(root.tag).toBe('variant');
    expect(root.cases.map((c) => c.name)).toEqual(['oidc', 'agent', 'golem-user', 'anonymous']);
  });

  it('round-trips a full OidcPrincipal', () => {
    const codec = compileSchema(s.principal());
    const input: Principal = sdkPrincipalFromHost({
      tag: 'oidc',
      val: {
        sub: 'user-123',
        issuer: 'https://issuer.example',
        email: 'a@b.com',
        emailVerified: true,
        givenName: 'Ada',
        claims: '{"role":"admin"}',
        // name / familyName / picture / preferredUsername intentionally omitted (undefined)
      },
    });

    const wire = codec.toValue(input) as { tag: string; caseIndex: number };
    expect(wire.tag).toBe('variant');
    expect(wire.caseIndex).toBe(0);

    const decoded = codec.fromValue(wire) as Principal;
    expect(decoded.tag).toBe('oidc');
    if (decoded.tag !== 'oidc') throw new Error('unreachable');
    expect(decoded.sub).toBe('user-123');
    expect(decoded.issuer).toBe('https://issuer.example');
    expect(decoded.email).toBe('a@b.com');
    expect(decoded.emailVerified).toBe(true);
    expect(decoded.givenName).toBe('Ada');
    expect(decoded.claims).toBe('{"role":"admin"}');
    expect(decoded.name).toBeUndefined();
    expect(decoded.familyName).toBeUndefined();
    expect(decoded.picture).toBeUndefined();
    expect(decoded.preferredUsername).toBeUndefined();
  });

  it('round-trips an AnonymousPrincipal', () => {
    const codec = compileSchema(s.principal());
    const input: Principal = sdkPrincipalFromHost({ tag: 'anonymous' });

    const wire = codec.toValue(input) as { tag: string; caseIndex: number; payload?: unknown };
    expect(wire.caseIndex).toBe(3);
    expect(wire.payload).toBeUndefined();

    const decoded = codec.fromValue(wire) as Principal;
    expect(decoded.tag).toBe('anonymous');
  });

  it('round-trips an AgentPrincipal (nested uuid/componentId/agentId)', () => {
    const codec = compileSchema(s.principal());
    const input: Principal = sdkPrincipalFromHost({
      tag: 'agent',
      val: {
        agentId: {
          componentId: { uuid: { highBits: 1n, lowBits: 2n } },
          agentId: 'agent-abc',
        },
      },
    });

    const wire = codec.toValue(input) as { tag: string; caseIndex: number };
    expect(wire.caseIndex).toBe(1);

    const decoded = codec.fromValue(wire) as Principal;
    expect(decoded.tag).toBe('agent');
    if (decoded.tag !== 'agent') throw new Error('unreachable');
    expect(decoded.agentId.agentId).toBe('agent-abc');
    expect(decoded.agentId.componentId.uuid).toEqual({ highBits: 1n, lowBits: 2n });
  });

  it('round-trips a GolemUserPrincipal', () => {
    const codec = compileSchema(s.principal());
    const input: Principal = sdkPrincipalFromHost({
      tag: 'golem-user',
      val: { accountId: { uuid: { highBits: 7n, lowBits: 9n } } },
    });

    const wire = codec.toValue(input) as { tag: string; caseIndex: number };
    expect(wire.caseIndex).toBe(2);

    const decoded = codec.fromValue(wire) as Principal;
    expect(decoded.tag).toBe('golem-user');
    if (decoded.tag !== 'golem-user') throw new Error('unreachable');
    expect(decoded.accountId.uuid).toEqual({ highBits: 7n, lowBits: 9n });
  });
});

describe('fluent s.principal() in agent-type assembly', () => {
  it('assembles an agent-type with s.principal() nested in a return object', async () => {
    const { defineAgent } = await import('../src/fluent/defineAgent');
    const { method } = await import('../src/fluent/method');
    const { z } = await import('zod');
    expect(() =>
      defineAgent({
        name: 'PrincipalReproAgent',
        id: { name: z.string() },
        methods: { echo: method({ input: {}, returns: z.object({ value: s.principal() }) }) },
      }),
    ).not.toThrow();
  });
});

// A bare `s.principal()` PARAMETER is auto-injected: the host supplies the caller
// principal, so the field carries WIT source `auto-injected(principal)` (no wire
// slot), while a normal param stays `user-supplied`. Mirrors the base SDK.
describe('s.principal() as an auto-injected parameter', () => {
  it('emits field-source auto-injected(principal) only for the bare principal param', async () => {
    const { defineAgent } = await import('../src/fluent/defineAgent');
    const { method } = await import('../src/fluent/method');
    const { z } = await import('zod');
    const { AgentTypeRegistry } = await import('../src/internal/registry/agentTypeRegistry');
    const { AgentClassName } = await import('../src/agentClassName');
    defineAgent({
      name: 'AutoInjectPrincipalAgent',
      id: { name: z.string() },
      methods: {
        whoAmI: method({
          input: { label: z.string(), caller: s.principal() },
          returns: z.string(),
        }),
      },
    });
    const at = AgentTypeRegistry.get(new AgentClassName('AutoInjectPrincipalAgent'))!;
    const m = at.methods.find((x) => x.name === 'whoAmI')!;
    const fields = (
      m.inputSchema as { tag: 'parameters'; val: Array<{ name: string; source: unknown }> }
    ).val;
    expect(fields.find((f) => f.name === 'label')!.source).toEqual({ tag: 'user-supplied' });
    expect(fields.find((f) => f.name === 'caller')!.source).toEqual({
      tag: 'auto-injected',
      val: 'principal',
    });
  });
});
