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
import { z } from 'zod';
import { defineAgent, method } from '../src/fluent';
import { compileSchema } from '../src/fluent/schema/adapter';
import { AgentClassName } from '../src/agentClassName';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';

describe('fluent Zod walker', () => {
  it('maps primitive schemas to schema types and round-trips values', () => {
    const s = compileSchema(z.string());
    expect(s.graph.root.body).toMatchObject({ tag: 'string' });
    expect(s.toValue('hi')).toEqual({ tag: 'string', value: 'hi' });
    expect(s.fromValue({ tag: 'string', value: 'hi' })).toBe('hi');

    const n = compileSchema(z.number());
    expect(n.graph.root.body).toMatchObject({ tag: 'f64' });
    expect(n.toValue(5)).toEqual({ tag: 'f64', value: 5 });
    expect(n.fromValue({ tag: 'f64', value: 8 })).toBe(8);

    expect(compileSchema(z.boolean()).graph.root.body).toMatchObject({ tag: 'bool' });
    expect(compileSchema(z.bigint()).graph.root.body).toMatchObject({ tag: 'u64' });
  });

  it('maps optional/nullable to option and unwraps default', () => {
    const opt = compileSchema(z.number().optional());
    expect(opt.graph.root.body).toMatchObject({ tag: 'option' });
    expect(opt.toValue(undefined)).toEqual({ tag: 'option', value: undefined });
    expect(opt.toValue(3)).toEqual({ tag: 'option', value: { tag: 'f64', value: 3 } });
    expect(opt.fromValue({ tag: 'option', value: undefined })).toBeUndefined();
    // `.default()` is transparent at the wire level.
    expect(compileSchema(z.number().int().default(1)).graph.root.body).toMatchObject({ tag: 'f64' });
  });

  it('maps arrays element-wise', () => {
    const arr = compileSchema(z.array(z.string()));
    expect(arr.graph.root.body).toMatchObject({ tag: 'list', element: { body: { tag: 'string' } } });
    expect(arr.fromValue(arr.toValue(['a', 'b']))).toEqual(['a', 'b']);
  });

  it('maps objects to records preserving field order', () => {
    const obj = compileSchema(z.object({ x: z.string(), y: z.number() }));
    expect(obj.graph.root.body).toMatchObject({ tag: 'record' });
    const fields = (obj.graph.root.body as { tag: 'record'; fields: { name: string }[] }).fields;
    expect(fields.map((f) => f.name)).toEqual(['x', 'y']);
    const value = { x: 'hi', y: 2 };
    expect(obj.fromValue(obj.toValue(value))).toEqual(value);
  });

  it('maps tuples element-wise', () => {
    const tup = compileSchema(z.tuple([z.string(), z.number(), z.boolean()]));
    expect(tup.graph.root.body).toMatchObject({ tag: 'tuple' });
    const value = ['a', 1, true];
    expect(tup.fromValue(tup.toValue(value))).toEqual(value);
  });

  it('maps string enums to enum nodes by case index', () => {
    const en = compileSchema(z.enum(['red', 'green', 'blue']));
    expect(en.graph.root.body).toMatchObject({ tag: 'enum', cases: ['red', 'green', 'blue'] });
    expect(en.toValue('green')).toEqual({ tag: 'enum', caseIndex: 1 });
    expect(en.fromValue({ tag: 'enum', caseIndex: 2 })).toBe('blue');
  });

  it('maps a literal to its base primitive', () => {
    const lit = compileSchema(z.literal('ok'));
    expect(lit.graph.root.body).toMatchObject({ tag: 'string' });
    expect(lit.fromValue(lit.toValue('ok'))).toBe('ok');
  });

  it('maps z.record to a map node', () => {
    const rec = compileSchema(z.record(z.string(), z.number()));
    expect(rec.graph.root.body).toMatchObject({ tag: 'map' });
    const value = { a: 1, b: 2 };
    expect(rec.fromValue(rec.toValue(value))).toEqual(value);
  });

  it('maps z.map to a map node (arbitrary keys)', () => {
    const m = compileSchema(z.map(z.string(), z.number()));
    expect(m.graph.root.body).toMatchObject({ tag: 'map' });
    const value = new Map([
      ['a', 1],
      ['b', 2],
    ]);
    expect(m.fromValue(m.toValue(value))).toEqual(value);
  });

  it('maps a discriminated union to a variant', () => {
    const du = compileSchema(
      z.discriminatedUnion('kind', [
        z.object({ kind: z.literal('a'), x: z.string() }),
        z.object({ kind: z.literal('b'), y: z.number() }),
      ]),
    );
    expect(du.graph.root.body).toMatchObject({ tag: 'variant' });
    const a = { kind: 'a' as const, x: 'hi' };
    const b = { kind: 'b' as const, y: 7 };
    expect(du.toValue(a)).toMatchObject({ tag: 'variant', caseIndex: 0 });
    expect(du.fromValue(du.toValue(a))).toEqual(a);
    expect(du.toValue(b)).toMatchObject({ tag: 'variant', caseIndex: 1 });
    expect(du.fromValue(du.toValue(b))).toEqual(b);
  });

  it('rejects non-Standard-Schema values', () => {
    expect(() => compileSchema({} as never)).toThrow(/Standard Schema/);
  });
});

describe('fluent defineAgent', () => {
  it('registers a well-formed AgentType for the counter', () => {
    const counterDef = defineAgent({
      name: 'counter',
      id: { name: z.string() },
      methods: {
        increment: method({ input: { by: z.number() }, returns: z.number() }),
        current: method({ input: {}, returns: z.number() }),
      },
    });

    counterDef.implement({
      init: () => ({ count: 0 }),
      methods: {
        increment({ by }) {
          this.count += by;
          return this.count;
        },
        current() {
          return this.count;
        },
      },
    });

    const agentType = AgentTypeRegistry.get(new AgentClassName('counter'));
    expect(agentType).toBeDefined();
    expect(agentType!.typeName).toBe('counter');
    expect(agentType!.sourceLanguage).toBe('typescript');

    // Constructor: single identity parameter `name`.
    const ctorInput = agentType!.constructor.inputSchema;
    expect(ctorInput.tag).toBe('parameters');
    expect(
      (ctorInput as { tag: 'parameters'; val: { name: string }[] }).val.map((f) => f.name),
    ).toEqual(['name']);

    // Methods: `increment(by)` returns a value; `current()` returns a value.
    const methods = Object.fromEntries(agentType!.methods.map((m) => [m.name, m]));
    expect(Object.keys(methods).sort()).toEqual(['current', 'increment']);

    const increment = methods['increment'];
    expect(increment.inputSchema.tag).toBe('parameters');
    expect(
      (increment.inputSchema as { tag: 'parameters'; val: { name: string }[] }).val.map(
        (f) => f.name,
      ),
    ).toEqual(['by']);
    expect(increment.outputSchema.tag).toBe('single');

    const current = methods['current'];
    expect((current.inputSchema as { tag: 'parameters'; val: unknown[] }).val).toEqual([]);
    expect(current.outputSchema.tag).toBe('single');

    // The initiator was registered so the guest can instantiate the agent.
    expect(AgentInitiatorRegistry.exists('counter')).toBe(true);
  });

  it('accepts an async init and exposes id/principal helpers on `this`', () => {
    const asyncCounter = defineAgent({
      name: 'asyncCounter',
      id: { name: z.string() },
      methods: {
        whoami: method({ input: {}, returns: z.string() }),
      },
    });

    asyncCounter.implement({
      // async `init` is now supported (may return State | Promise<State>).
      init: async () => ({ count: 0 }),
      methods: {
        whoami() {
          // Identity + principal helpers are available on `this`.
          const id = this.getId();
          void this.getPhantomId();
          void this.getPrincipal();
          return id.value;
        },
      },
    });

    expect(AgentInitiatorRegistry.exists('asyncCounter')).toBe(true);
  });
});
