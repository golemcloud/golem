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
import { defineAgent } from '../src/fluent/defineAgent';
import { method } from '../src/fluent/method';
import { AgentClassName } from '../src/agentClassName';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';

const get = (name: string) => AgentTypeRegistry.get(new AgentClassName(name));

describe('fluent agent metadata (Phase 3)', () => {
  it('propagates agent description and promptHint into the AgentType', () => {
    defineAgent({
      name: 'metaDescribed',
      description: 'A well-described agent',
      promptHint: 'Provide the name to identify the agent',
      id: { name: z.string() },
      methods: {
        ping: method({ input: {}, returns: z.string() }),
      },
    });

    const at = get('metaDescribed');
    expect(at).toBeDefined();
    expect(at!.description).toBe('A well-described agent');
    // promptHint is surfaced on the constructor.
    expect(at!.constructor.promptHint).toBe('Provide the name to identify the agent');
  });

  it('defaults mode to durable and ephemeral mode maps to the ephemeral WIT mode', () => {
    defineAgent({
      name: 'durableByDefault',
      id: { name: z.string() },
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    expect(get('durableByDefault')!.mode).toBe('durable');

    defineAgent({
      name: 'ephemeralAgent',
      mode: 'ephemeral',
      id: { name: z.string() },
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    expect(get('ephemeralAgent')!.mode).toBe('ephemeral');
  });

  it('propagates per-method description / promptHint / readOnly', () => {
    defineAgent({
      name: 'methodMeta',
      id: { name: z.string() },
      methods: {
        read: method({
          input: {},
          returns: z.string(),
          description: 'Reads the value',
          promptHint: 'No arguments needed',
          readOnly: true,
        }),
        write: method({ input: { v: z.string() }, returns: z.void() }),
      },
    });

    const methods = Object.fromEntries(get('methodMeta')!.methods.map((m) => [m.name, m]));

    expect(methods['read'].description).toBe('Reads the value');
    expect(methods['read'].promptHint).toBe('No arguments needed');
    // `readOnly: true` uses the `until-write` cache policy (matching the base SDK).
    expect(methods['read'].readOnly).toEqual({
      cachePolicy: { tag: 'until-write' },
      usesPrincipal: false,
    });

    // Omitted metadata keeps today's defaults.
    expect(methods['write'].description).toBe('');
    expect(methods['write'].promptHint).toBeUndefined();
    expect(methods['write'].readOnly).toBeUndefined();
  });

  it('maps snapshotting variants correctly', () => {
    defineAgent({
      name: 'snapDefault',
      snapshotting: 'default',
      id: { name: z.string() },
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    expect(get('snapDefault')!.snapshotting).toEqual({ tag: 'enabled', val: { tag: 'default' } });

    defineAgent({
      name: 'snapPeriodic',
      snapshotting: { periodicSeconds: 30 },
      id: { name: z.string() },
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    expect(get('snapPeriodic')!.snapshotting).toEqual({
      tag: 'enabled',
      val: { tag: 'periodic', val: 30_000_000_000n },
    });

    defineAgent({
      name: 'snapEveryN',
      snapshotting: { everyNInvocations: 5 },
      id: { name: z.string() },
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    expect(get('snapEveryN')!.snapshotting).toEqual({
      tag: 'enabled',
      val: { tag: 'every-n-invocation', val: 5 },
    });

    defineAgent({
      name: 'snapDisabled',
      snapshotting: 'disabled',
      id: { name: z.string() },
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    expect(get('snapDisabled')!.snapshotting).toEqual({ tag: 'disabled' });
  });

  it('emits an agent-dependency record from a declared dependency', () => {
    const childDef = defineAgent({
      name: 'depChild',
      description: 'The child dependency',
      id: { childId: z.string() },
      methods: {
        childMethod: method({ input: { x: z.number() }, returns: z.number() }),
      },
    });

    defineAgent({
      name: 'depParent',
      dependencies: [childDef],
      id: { name: z.string() },
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });

    const parent = get('depParent')!;
    expect(parent.dependencies).toHaveLength(1);
    const dep = parent.dependencies[0];
    expect(dep.typeName).toBe('depChild');
    expect(dep.description).toBe('The child dependency');
    // Reuses the dependency's registered constructor + methods.
    expect(dep.methods.map((m) => m.name)).toEqual(['childMethod']);
    const child = get('depChild')!;
    expect(dep.schema).toBe(child.schema);
    expect(dep.constructor).toBe(child.constructor);
  });

  it('throws a clear error when a dependency is not yet registered', () => {
    expect(() =>
      defineAgent({
        name: 'depMissingParent',
        dependencies: [{ name: 'neverRegistered' } as never],
        id: { name: z.string() },
        methods: { ping: method({ input: {}, returns: z.string() }) },
      }),
    ).toThrow(/neverRegistered/);
  });

  it("registers an agent with NO metadata using today's defaults", () => {
    defineAgent({
      name: 'noMeta',
      id: { name: z.string() },
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });

    const at = get('noMeta')!;
    expect(at).toBeDefined();
    expect(at.mode).toBe('durable');
    expect(at.snapshotting).toEqual({ tag: 'disabled' });
    expect(at.dependencies).toEqual([]);
    expect(at.config).toEqual([]);
    expect(at.httpMount).toBeUndefined();
    // The agent-type description defaults to the constructor description.
    expect(at.description).toBe('Constructs the agent noMeta');
  });
});
