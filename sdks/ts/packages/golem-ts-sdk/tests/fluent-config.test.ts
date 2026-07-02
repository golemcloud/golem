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

// Phase 4: agent CONFIG (local + secret) on the Standard-Schema path.
//
// The live runtime accessor (`buildConfigAccessor`) calls host bindings
// (`getConfigValue` / `reveal`) that only resolve inside the Golem guest, so
// these tests never invoke it. Instead they assert the *declarative* output:
//   - `defineAgent({ config })` emits `agent-config-declaration`s whose
//     `valueType` resolves (in the agent-type `schema`) to the right node, and
//   - `compileConfig` produces the expected per-field declaration graphs.

import { describe, it, expect } from 'vitest';
import { z } from 'zod';
import { defineAgent } from '../src/fluent/defineAgent';
import { method } from '../src/fluent/method';
import { compileConfig } from '../src/fluent/config';
import { AgentClassName } from '../src/agentClassName';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';

const get = (name: string) => AgentTypeRegistry.get(new AgentClassName(name));

describe('fluent agent config (Phase 4)', () => {
  it('emits agent-config-declarations whose value-types resolve to the right schema nodes', () => {
    defineAgent({
      name: 'configured',
      id: { name: z.string() },
      methods: { ping: method({ input: {}, returns: z.string() }) },
      config: {
        local: { greeting: z.string() },
        secret: { apiKey: z.string() },
      },
    });

    const at = get('configured')!;
    expect(at).toBeDefined();
    expect(at.config).toHaveLength(2);

    const byPath = Object.fromEntries(at.config.map((c) => [c.path.join('.'), c]));

    const local = byPath['greeting'];
    expect(local).toBeDefined();
    expect(local.source).toBe('local');
    // Local field's value-type resolves to a plain string node.
    expect(at.schema.typeNodes[local.valueType].body.tag).toBe('string-type');

    const secret = byPath['apiKey'];
    expect(secret).toBeDefined();
    expect(secret.source).toBe('secret');
    // Secret field's value-type resolves to a `secret-type` capability node...
    const secretNode = at.schema.typeNodes[secret.valueType].body;
    expect(secretNode.tag).toBe('secret-type');
    // ...whose inner (revealed) payload is the plain string node.
    const innerIndex = (secretNode as { tag: 'secret-type'; val: { inner: number } }).val.inner;
    expect(at.schema.typeNodes[innerIndex].body.tag).toBe('string-type');
  });

  it('compileConfig produces local + secret declarations with the right graphs', () => {
    const declarations = compileConfig({
      local: { greeting: z.string() },
      secret: { apiKey: z.string() },
    });

    expect(declarations).toHaveLength(2);

    const byName = Object.fromEntries(declarations.map((d) => [d.name, d]));

    const local = byName['greeting'];
    expect(local.source).toBe('local');
    expect(local.path).toEqual(['greeting']);
    // The declaration graph root is the inner (string) type directly.
    expect(local.graph.root.body.tag).toBe('string');

    const secret = byName['apiKey'];
    expect(secret.source).toBe('secret');
    expect(secret.path).toEqual(['apiKey']);
    // The declaration graph root is wrapped in a `secret` node...
    expect(secret.graph.root.body.tag).toBe('secret');
    // ...whose inner payload (and the codec) drive the plaintext string value.
    const inner = (secret.graph.root.body as { tag: 'secret'; inner: { body: { tag: string } } })
      .inner;
    expect(inner.body.tag).toBe('string');
    expect(secret.codec.graph.root.body.tag).toBe('string');
  });

  it('an agent with no config keeps an empty config declaration list', () => {
    defineAgent({
      name: 'noConfig',
      id: { name: z.string() },
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    expect(get('noConfig')!.config).toEqual([]);
    expect(compileConfig(undefined)).toEqual([]);
  });
});
