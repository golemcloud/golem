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

// Agent CONFIG on the Standard-Schema path: a single record of fields, with
// secrets marked by `s.secret(inner)` (no more `{ local, secret }` buckets).
//
// The live runtime accessor (`buildConfigAccessor`) calls host bindings
// (`getConfigValue` / `reveal`) that only resolve inside the Golem guest, so
// these tests never invoke it. Instead they assert the *declarative* output:
//   - `defineAgent({ config })` emits `agent-config-declaration`s whose
//     `valueType` resolves (in the agent-type `schema`) to the right node, and
//   - `compileConfig` produces the expected per-field declaration graphs, with
//     secret vs local detected by the `s.secret(...)` marker.

import { describe, it, expect } from 'vitest';
import { z } from 'zod';
import { defineAgent } from '../src/fluent/defineAgent';
import { method } from '../src/fluent/method';
import { compileConfig } from '../src/fluent/config';
import { s } from '../src/fluent/schema/markers';
import { Secret } from '../src/fluent/secret';
import { AgentClassName } from '../src/agentClassName';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';

const get = (name: string) => AgentTypeRegistry.get(new AgentClassName(name));

describe('fluent agent config (single-record + s.secret marker)', () => {
  it('emits agent-config-declarations whose value-types resolve to the right schema nodes', () => {
    defineAgent({
      name: 'configured',
      id: { name: z.string() },
      methods: { ping: method({ input: {}, returns: z.string() }) },
      config: {
        greeting: z.string(),
        apiKey: s.secret(z.string()),
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

  it('compileConfig detects secret vs local by the s.secret marker', () => {
    const declarations = compileConfig({
      greeting: z.string(),
      apiKey: s.secret(z.string()),
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

  it('a plain field (no marker) compiles as local', () => {
    const [decl] = compileConfig({ greeting: z.string() });
    expect(decl.source).toBe('local');
  });

  it('flattens nested objects to leaf declarations with full multi-segment paths', () => {
    const declarations = compileConfig({
      top: z.string(),
      nested: z.object({
        a: z.string(),
        b: z.number(),
        c: s.secret(z.object({ d: z.string(), e: z.number() })),
      }),
    });

    const byPath = Object.fromEntries(declarations.map((d) => [d.path.join('.'), d]));

    // Top-level local leaf.
    expect(byPath['top'].source).toBe('local');
    expect(byPath['top'].path).toEqual(['top']);

    // Nested local leaves carry their full path.
    expect(byPath['nested.a'].source).toBe('local');
    expect(byPath['nested.a'].path).toEqual(['nested', 'a']);
    expect(byPath['nested.a'].graph.root.body.tag).toBe('string');
    expect(byPath['nested.b'].source).toBe('local');
    expect(byPath['nested.b'].path).toEqual(['nested', 'b']);

    // Nested secret leaf: full path, `secret<inner>` declaration graph, inner codec.
    const secret = byPath['nested.c'];
    expect(secret.source).toBe('secret');
    expect(secret.path).toEqual(['nested', 'c']);
    expect(secret.graph.root.body.tag).toBe('secret');
    // The inner codec decodes the revealed plaintext (the { d, e } record).
    expect(secret.codec.graph.root.body.tag).toBe('record');

    // No declaration for the intermediate `nested` object itself — only leaves.
    expect(byPath['nested']).toBeUndefined();
  });

  it('reads a whole array/union field as a single local leaf (no recursion)', () => {
    const declarations = compileConfig({ tags: z.array(z.string()) });
    expect(declarations).toHaveLength(1);
    expect(declarations[0].source).toBe('local');
    expect(declarations[0].path).toEqual(['tags']);
    expect(declarations[0].graph.root.body.tag).toBe('list');
  });

  it('Secret.toJSON throws so secrets never leak through serialization', () => {
    const [secretDecl] = compileConfig({ apiKey: s.secret(z.string()) });
    const handle = new Secret(secretDecl);
    expect(() => JSON.stringify(handle)).toThrow(/not serializable/);
    expect(() => handle.toJSON()).toThrow(/not serializable/);
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
