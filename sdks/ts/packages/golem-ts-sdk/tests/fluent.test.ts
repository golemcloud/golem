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
});
