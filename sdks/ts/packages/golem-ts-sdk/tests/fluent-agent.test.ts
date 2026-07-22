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

import { describe, it, expect, vi } from 'vitest';
import { z } from 'zod';
import { defineAgent } from '../src/fluent/defineAgent';
import { method } from '../src/fluent/method';
import { AgentClassName } from '../src/agentClassName';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { schemaValueFromWit, schemaValueToWit, v } from '../src/internal/schema-model';
import { guest } from '../src';

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

  it('defers a clear error when a dependency is not yet registered', () => {
    expect(() =>
      defineAgent({
        name: 'depMissingParent',
        dependencies: [{ name: 'neverRegistered' } as never],
        id: { name: z.string() },
        methods: { ping: method({ input: {}, returns: z.string() }) },
      }),
    ).not.toThrow();
    expect(get('depMissingParent')).toBeUndefined();
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

  it('rejects snapshot restoration for an agent with a deferred registration failure', async () => {
    vi.resetModules();
    const [{ defineAgent: isolatedDefineAgent }, { method: isolatedMethod }, isolatedGuest] =
      await Promise.all([
        import('../src/fluent/defineAgent'),
        import('../src/fluent/method'),
        import('../src'),
      ]);

    const invalidDef = isolatedDefineAgent({
      name: 'SnapshotDeferredInvalid',
      id: {},
      methods: { ping: isolatedMethod({ input: {}, returns: z.string() }) },
    });
    const implementation = {
      init: () => ({}),
      methods: { ping: () => 'ok' },
      snapshot: {
        save: () => new Uint8Array(),
        load: () => undefined,
      },
    };
    invalidDef.implement(implementation);
    invalidDef.implement(implementation);

    const emptyInput = schemaValueToWit(v.record([]));
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `SnapshotDeferredInvalid(${JSON.stringify(emptyInput)})`;

    await expect(
      isolatedGuest.loadSnapshot.load({
        payload: new Uint8Array([1]),
        mimeType: 'application/octet-stream',
      }),
    ).rejects.toContain('implement() was called more than once');
  });

  it('reports a deferred registration failure before decoding an affected agent snapshot', async () => {
    vi.resetModules();
    const [{ defineAgent: isolatedDefineAgent }, { method: isolatedMethod }, isolatedGuest] =
      await Promise.all([
        import('../src/fluent/defineAgent'),
        import('../src/fluent/method'),
        import('../src'),
      ]);

    const invalidDef = isolatedDefineAgent({
      name: 'MalformedSnapshotDeferredInvalid',
      id: {},
      methods: { ping: isolatedMethod({ input: {}, returns: z.string() }) },
    });
    const implementation = {
      init: () => ({}),
      methods: { ping: () => 'ok' },
    };
    invalidDef.implement(implementation);
    invalidDef.implement(implementation);

    const emptyInput = schemaValueToWit(v.record([]));
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `MalformedSnapshotDeferredInvalid(${JSON.stringify(emptyInput)})`;

    const rejection = await isolatedGuest.loadSnapshot
      .load({
        payload: new TextEncoder().encode('{'),
        mimeType: 'application/json',
      })
      .then(
        () => undefined,
        (error: unknown) => error,
      );

    expect(typeof rejection).toBe('string');
    expect(rejection).toContain('MalformedSnapshotDeferredInvalid');
    expect(rejection).toContain('implement() was called more than once');
  });

  it('attributes deferred implementation failures to the agent definition name', async () => {
    vi.resetModules();
    const {
      defineAgent: isolatedDefineAgent,
      method: isolatedMethod,
      AgentTypeRegistry,
    } = await import('../src');

    const spec = {
      name: 'OriginalAgentName',
      id: {},
      methods: { ping: isolatedMethod({ input: {}, returns: z.string() }) },
    };
    const originalDef = isolatedDefineAgent(spec);
    const implementation = {
      init: () => ({}),
      methods: { ping: () => 'original' },
    };
    originalDef.implement(implementation);

    isolatedDefineAgent({
      name: 'UnrelatedAgentName',
      id: {},
      methods: { ping: isolatedMethod({ input: {}, returns: z.string() }) },
    }).implement({
      init: () => ({}),
      methods: { ping: () => 'unrelated' },
    });

    spec.name = 'UnrelatedAgentName';
    originalDef.implement(implementation);

    expect(AgentTypeRegistry.getRegistrationError('OriginalAgentName')).toEqual([
      expect.stringContaining('implement() was called more than once'),
    ]);
    expect(AgentTypeRegistry.getRegistrationError('UnrelatedAgentName')).toBeUndefined();
  });

  it('does not silently overwrite a re-entrant duplicate definition', async () => {
    vi.resetModules();
    const {
      defineAgent: isolatedDefineAgent,
      method: isolatedMethod,
      AgentTypeRegistry,
      AgentClassName: IsolatedAgentClassName,
    } = await import('../src');

    let nested = false;
    const id = {} as Record<string, z.ZodType>;
    Object.defineProperty(id, 'key', {
      enumerable: true,
      get() {
        if (!nested) {
          nested = true;
          isolatedDefineAgent({
            name: 'ReentrantDuplicateDefinition',
            id: {},
            methods: {
              inner: isolatedMethod({ input: {}, returns: z.string() }),
            },
          });
        }
        return z.string();
      },
    });

    isolatedDefineAgent({
      name: 'ReentrantDuplicateDefinition',
      id,
      methods: {
        outer: isolatedMethod({ input: {}, returns: z.string() }),
      },
    });

    const registered = AgentTypeRegistry.get(
      new IsolatedAgentClassName('ReentrantDuplicateDefinition'),
    );
    expect(registered?.methods.map((registeredMethod) => registeredMethod.name)).toEqual(['outer']);
    expect(AgentTypeRegistry.getRegistrationError('ReentrantDuplicateDefinition')).toEqual([
      expect.stringContaining('already registered'),
    ]);
  });

  it('surfaces deferred definition and implementation failures as typed guest errors', async () => {
    const invalidDef = defineAgent({
      name: 'DeferredInvalidHttp',
      id: {},
      http: { path: 'missing-leading-slash' },
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    expect(() =>
      invalidDef.implement({
        init: () => ({}),
        methods: { ping: () => 'invalid' },
      }),
    ).not.toThrow();
    expect(get('DeferredInvalidHttp')).toBeUndefined();

    const duplicateImplDef = defineAgent({
      name: 'DeferredDuplicateImpl',
      id: {},
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    const duplicateImpl = {
      init: () => ({}),
      methods: { ping: () => 'duplicate' },
    };
    duplicateImplDef.implement(duplicateImpl);
    expect(() => duplicateImplDef.implement(duplicateImpl)).not.toThrow();

    const validDef = defineAgent({
      name: 'DeferredValidAgent',
      id: {},
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    validDef.implement({
      init: () => ({}),
      methods: { ping: () => 'valid' },
    });

    const discoveryError = await guest.discoverAgentTypes().then(
      () => undefined,
      (error: unknown) => error,
    );
    expect(discoveryError).toMatchObject({ tag: 'custom-error' });
    const discoveryValue = schemaValueFromWit(
      (discoveryError as { val: { value: Parameters<typeof schemaValueFromWit>[0] } }).val.value,
    );
    expect(discoveryValue).toMatchObject({ tag: 'string' });
    if (discoveryValue.tag !== 'string') throw new Error('expected string custom error');
    expect(discoveryValue.value).toContain('depMissingParent');
    expect(discoveryValue.value).toContain('DeferredInvalidHttp');
    expect(discoveryValue.value).toContain('DeferredDuplicateImpl');

    const emptyInput = schemaValueToWit(v.record([]));
    const invalidInitializeError = await guest
      .initialize('DeferredInvalidHttp', emptyInput, { tag: 'anonymous' })
      .then(
        () => undefined,
        (error: unknown) => error,
      );
    expect(invalidInitializeError).toMatchObject({ tag: 'custom-error' });

    (globalThis as { currentAgentId?: string }).currentAgentId =
      `DeferredValidAgent(${JSON.stringify(emptyInput)})`;
    await expect(
      guest.initialize('DeferredValidAgent', emptyInput, { tag: 'anonymous' }),
    ).resolves.toBeUndefined();
    await expect(guest.getDefinition()).resolves.toBe(get('DeferredValidAgent'));
  });
});
