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
import { compileConfig, compileConfigTree, ConfigGroupNode } from '../src/fluent/config';
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

  it('preserves null when decoding a nullable config leaf', () => {
    const [declaration] = compileConfig({ label: z.string().nullable() });
    expect(declaration.codec.fromValue(declaration.codec.toValue(null))).toBeNull();
  });

  it('preserves null when decoding a nullable object-valued config field', () => {
    const declarations = compileConfig({ group: z.object({ label: z.string() }).nullable() });
    expect(declarations).toHaveLength(1);
    expect(declarations[0].path).toEqual(['group']);
    expect(declarations[0].codec.fromValue(declarations[0].codec.toValue(null))).toBeNull();
  });

  it('Secret.toJSON throws so secrets never leak through serialization', () => {
    const [secretDecl] = compileConfig({ apiKey: s.secret(z.string()) });
    const handle = new Secret(secretDecl);
    expect(() => JSON.stringify(handle)).toThrow(/not serializable/);
    expect(() => handle.toJSON()).toThrow(/not serializable/);
  });

  // --- OPTIONAL object groups (z.object({...}).optional()) ---

  // The `element` of an `option<...>` declaration graph root's body.
  const optionElement = (graph: { root: { body: unknown } }): { body: { tag: string } } => {
    const body = graph.root.body as { tag: string; element: { body: { tag: string } } };
    expect(body.tag).toBe('option');
    return body.element;
  };

  it('descends an OPTIONAL object group into per-leaf declarations, lifting required leaves to option', () => {
    const declarations = compileConfig({
      required: z.string(),
      optionalGroup: z.object({ a: z.number(), b: z.string().optional() }).optional(),
    });

    const byPath = Object.fromEntries(declarations.map((d) => [d.path.join('.'), d]));

    // No whole-group leaf — the group is descended into its children.
    expect(byPath['optionalGroup']).toBeUndefined();

    // Top-level required leaf: plain string, required.
    expect(byPath['required'].source).toBe('local');
    expect(byPath['required'].graph.root.body.tag).toBe('string');
    expect(byPath['required'].required).toBe(true);

    // Required child `a` (z.number()) is LIFTED to option<f64> so an unset value
    // reads as none instead of trapping — but it still counts as required.
    const a = byPath['optionalGroup.a'];
    expect(a.source).toBe('local');
    expect(a.path).toEqual(['optionalGroup', 'a']);
    expect(optionElement(a.graph).body.tag).toBe('f64');
    expect(a.required).toBe(true);

    // Optional child `b` (already option<string>): not required, not double-wrapped.
    const b = byPath['optionalGroup.b'];
    expect(b.path).toEqual(['optionalGroup', 'b']);
    expect(optionElement(b.graph).body.tag).toBe('string');
    expect(b.required).toBe(false);
  });

  it('records optional-group presence (requiredKeys) in the compiled tree', () => {
    const tree = compileConfigTree({
      required: z.string(),
      optionalGroup: z.object({ a: z.number(), b: z.string().optional() }).optional(),
    });

    const group = tree.children.find(
      (c): c is ConfigGroupNode => c.kind === 'group' && c.name === 'optionalGroup',
    )!;
    expect(group).toBeDefined();
    expect(group.optional).toBe(true);
    // Only the non-optional child `a` gates the group's presence.
    expect(group.requiredKeys).toEqual(['a']);
  });

  it('distinguishes an absent optional group from a present required nullable child', () => {
    const spec = {
      group: z.object({ label: z.string().nullable() }).optional(),
    };
    const tree = compileConfigTree(spec);
    const group = tree.children[0] as ConfigGroupNode;
    expect(group.requiredKeys).toEqual(['label']);

    const [declaration] = compileConfig(spec);
    expect(declaration.required).toBe(true);
    expect(declaration.codec.fromValue({ tag: 'option', value: undefined })).toBeUndefined();
    expect(declaration.codec.fromValue(declaration.codec.toValue(null))).toBeNull();
  });

  it('an ALL-optional group has no required children (always present)', () => {
    const tree = compileConfigTree({
      allOptionalGroup: z.object({ x: z.number().optional(), y: z.string().optional() }).optional(),
    });
    const group = tree.children[0] as ConfigGroupNode;
    expect(group.kind).toBe('group');
    expect(group.optional).toBe(true);
    expect(group.requiredKeys).toEqual([]);

    // Both leaves are declared as options; neither is required.
    const declarations = compileConfig({
      allOptionalGroup: z.object({ x: z.number().optional(), y: z.string().optional() }).optional(),
    });
    const byPath = Object.fromEntries(declarations.map((d) => [d.path.join('.'), d]));
    expect(optionElement(byPath['allOptionalGroup.x'].graph).body.tag).toBe('f64');
    expect(byPath['allOptionalGroup.x'].required).toBe(false);
    expect(optionElement(byPath['allOptionalGroup.y'].graph).body.tag).toBe('string');
    expect(byPath['allOptionalGroup.y'].required).toBe(false);
  });

  it('a nested REQUIRED subgroup under an optional group gates presence transitively', () => {
    const spec = {
      outer: z.object({ required: z.string(), inner: z.object({ a: z.number() }) }).optional(),
    };

    const declarations = compileConfig(spec);
    const byPath = Object.fromEntries(declarations.map((d) => [d.path.join('.'), d]));
    // Both leaves sit under the optional `outer`, so both are option-lifted...
    expect(optionElement(byPath['outer.required'].graph).body.tag).toBe('string');
    expect(byPath['outer.required'].required).toBe(true);
    expect(byPath['outer.inner.a'].path).toEqual(['outer', 'inner', 'a']);
    expect(optionElement(byPath['outer.inner.a'].graph).body.tag).toBe('f64');
    expect(byPath['outer.inner.a'].required).toBe(true);

    const tree = compileConfigTree(spec);
    const outer = tree.children[0] as ConfigGroupNode;
    expect(outer.optional).toBe(true);
    // `inner` is a non-optional subgroup, so it counts toward `outer`'s presence.
    expect(outer.requiredKeys).toEqual(['required', 'inner']);
    const inner = outer.children.find(
      (c): c is ConfigGroupNode => c.kind === 'group' && c.name === 'inner',
    )!;
    expect(inner.optional).toBe(false);
    expect(inner.requiredKeys).toEqual(['a']);
  });

  it('keeps a NON-optional nested object descended with plain (non-lifted) leaves', () => {
    // Mirrors LocalConfigAgent.nested — a required group whose leaves stay plain.
    const declarations = compileConfig({
      nested: z.object({ a: z.boolean(), b: z.array(z.number()) }),
    });
    const byPath = Object.fromEntries(declarations.map((d) => [d.path.join('.'), d]));
    // Not under an optional ancestor → NOT lifted to option.
    expect(byPath['nested.a'].graph.root.body.tag).toBe('bool');
    expect(byPath['nested.a'].required).toBe(true);
    expect(byPath['nested.b'].graph.root.body.tag).toBe('list');
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
