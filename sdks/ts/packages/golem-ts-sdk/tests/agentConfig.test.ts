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

import { afterEach, describe, expect, it, vi } from 'vitest';
import { AgentClassName } from '../src';
import { buildTypeFromJSON, TypeMetadata } from '@golemcloud/golem-ts-types-core';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { paramNames, paramShape, schemaTypeAt, normalizeSchema } from './agentTypeHelpers';
import { resolveAgentConfig } from '../src/internal/schema/agentType';
import * as Either from '../src/newTypes/either';
import { r, resolvedField } from '../src/internal/mapping/types/resolvedType';

describe('agent config handling', () => {
  it('correctly describes a complex config type', () => {
    const configAgent = TypeMetadata.get('ConfigAgent')!;
    const constructorArgs = configAgent.constructorArgs;

    const arg = constructorArgs[1];
    expect(arg.name).toBe('config');
    expect(arg.type.optional).toBe(false);
    expect(arg.type.kind).toBe('config');
    assert(arg.type.kind === 'config');
    expect(arg.type.properties).toHaveLength(7);
    expect(arg.type.requiredMembers).toEqual(
      expect.arrayContaining([
        { path: ['nested'], requiredKeys: ['nestedSecret', 'a', 'b'] },
        { path: ['aliasedNested'], requiredKeys: ['c'] },
      ]),
    );
    expect(arg.type.properties).toEqual([
      {
        path: ['foo'],
        secret: false,
        secretHandleOptional: undefined,
        type: { kind: 'number', name: undefined, owner: undefined, optional: false },
      },
      {
        path: ['bar'],
        secret: false,
        secretHandleOptional: undefined,
        type: { kind: 'string', name: undefined, owner: undefined, optional: false },
      },
      {
        path: ['secret'],
        secret: true,
        secretHandleOptional: false,
        type: { kind: 'boolean', name: undefined, owner: undefined, optional: false },
      },
      {
        path: ['nested', 'nestedSecret'],
        secret: true,
        secretHandleOptional: false,
        type: { kind: 'number', name: undefined, owner: undefined, optional: false },
      },
      {
        path: ['nested', 'a'],
        secret: false,
        secretHandleOptional: undefined,
        type: { kind: 'boolean', name: undefined, owner: undefined, optional: false },
      },
      {
        path: ['nested', 'b'],
        secret: false,
        secretHandleOptional: undefined,
        type: {
          kind: 'array',
          name: undefined,
          owner: undefined,
          element: { kind: 'number', name: undefined, owner: undefined, optional: false },
          optional: false,
        },
      },
      {
        path: ['aliasedNested', 'c'],
        secret: false,
        secretHandleOptional: undefined,
        type: { kind: 'number', name: undefined, owner: undefined, optional: false },
      },
    ]);
  });

  it('correctly describes expected config entries to the host', () => {
    const configAgent = AgentTypeRegistry.get(new AgentClassName('ConfigAgent'))!;
    expect(configAgent.config).toHaveLength(7);

    // Each config declaration carries a `valueType` index into the agent's
    // shared schema graph; resolve it to assert source, path and value type.
    const resolved = configAgent.config.map((entry) => {
      const { root, defs } = schemaTypeAt(configAgent, entry.valueType);
      return {
        source: entry.source,
        path: entry.path,
        type: normalizeSchema(root, defs),
      };
    });

    expect(resolved).toEqual([
      { source: 'local', path: ['foo'], type: 'f64' },
      { source: 'local', path: ['bar'], type: 'string' },
      { source: 'secret', path: ['secret'], type: { secret: 'bool' } },
      { source: 'secret', path: ['nested', 'nestedSecret'], type: { secret: 'f64' } },
      { source: 'local', path: ['nested', 'a'], type: 'bool' },
      { source: 'local', path: ['nested', 'b'], type: { list: 'f64' } },
      { source: 'local', path: ['aliasedNested', 'c'], type: 'f64' },
    ]);
  });

  it('keeps only the secret handle optional for optional secret config entries', () => {
    const resolved = resolveAgentConfig([
      {
        name: 'config',
        type: {
          kind: 'config',
          optional: false,
          properties: [
            {
              path: ['optionalSecret'],
              secret: true,
              type: { kind: 'string', optional: true },
            },
          ],
          requiredMembers: [],
        },
      },
    ]);

    expect(Either.isRight(resolved)).toBe(true);
    if (!Either.isRight(resolved)) return;

    expect(resolved.val).toHaveLength(1);
    const graph = resolved.val[0].valueGraph;
    expect(normalizeSchema(graph.root, graph.defs)).toEqual({ option: { secret: 'string' } });
  });

  it('keeps payload optionality inside required secret config entries', () => {
    const resolved = resolveAgentConfig([
      {
        name: 'config',
        type: {
          kind: 'config',
          optional: false,
          properties: [
            {
              path: ['requiredMaybeSecret'],
              secret: true,
              type: buildTypeFromJSON({
                kind: 'union',
                optional: false,
                types: [
                  { kind: 'string', optional: false },
                  { kind: 'undefined', optional: false },
                ],
                typeParams: [],
                originalTypeName: undefined,
              }),
            },
          ],
          requiredMembers: [],
        },
      },
    ]);

    expect(Either.isRight(resolved)).toBe(true);
    if (!Either.isRight(resolved)) return;

    expect(resolved.val).toHaveLength(1);
    const graph = resolved.val[0].valueGraph;
    expect(normalizeSchema(graph.root, graph.defs)).toEqual({ secret: { option: 'string' } });
  });

  it('keeps payload optionality inside optional secret config entries', () => {
    const resolved = resolveAgentConfig([
      {
        name: 'config',
        type: {
          kind: 'config',
          optional: false,
          properties: [
            {
              path: ['optionalMaybeSecret'],
              secret: true,
              type: buildTypeFromJSON({
                kind: 'union',
                optional: true,
                types: [
                  { kind: 'string', optional: false },
                  { kind: 'undefined', optional: false },
                ],
                typeParams: [],
                originalTypeName: undefined,
              }),
            },
          ],
          requiredMembers: [],
        },
      },
    ]);

    expect(Either.isRight(resolved)).toBe(true);
    if (!Either.isRight(resolved)) return;

    expect(resolved.val).toHaveLength(1);
    const graph = resolved.val[0].valueGraph;
    expect(normalizeSchema(graph.root, graph.defs)).toEqual({
      option: { secret: { option: 'string' } },
    });
  });

  it('treats a required secret member inside an optional config group as an optional handle', () => {
    const resolved = resolveAgentConfig([
      {
        name: 'config',
        type: {
          kind: 'config',
          optional: false,
          properties: [
            {
              path: ['group', 'apiKey'],
              secret: true,
              type: { kind: 'string', optional: true },
            },
          ],
          requiredMembers: [{ path: ['group'], requiredKeys: ['apiKey'] }],
        },
      },
    ]);

    expect(Either.isRight(resolved)).toBe(true);
    if (!Either.isRight(resolved)) return;

    const graph = resolved.val[0].valueGraph;
    expect(normalizeSchema(graph.root, graph.defs)).toEqual({ option: { secret: 'string' } });
  });

  it('config parameters should not show up in declared constructor', () => {
    const configAgent = AgentTypeRegistry.get(new AgentClassName('ConfigAgent'))!;

    // `config` parameters are lifted into agent-level config declarations and
    // never consume a constructor input field; only `before` / `after` remain.
    expect(configAgent.constructor.inputSchema.tag).toBe('parameters');
    expect(paramNames(configAgent.constructor.inputSchema)).toEqual(['before', 'after']);

    expect(paramShape(configAgent, configAgent.constructor.inputSchema, 'before')).toEqual('f64');
    expect(paramShape(configAgent, configAgent.constructor.inputSchema, 'after')).toEqual('bool');
  });
});

describe('optional secret config runtime loading', () => {
  afterEach(() => {
    vi.doUnmock('golem:agent/host@2.0.0');
    vi.doUnmock('golem:secrets/reveal@0.1.0');
    vi.resetModules();
  });

  it('reveals a present optional secret config value encoded as option<secret<T>>', async () => {
    const rawSecret = { id: 'opaque-secret' } as never;
    const getConfigValue = vi.fn(() => ({
      valueNodes: [
        { tag: 'secret-value', val: rawSecret },
        { tag: 'option-value', val: 0 },
      ],
      root: 1,
    }));
    const reveal = vi.fn(() => ({
      valueNodes: [{ tag: 'string-value', val: 'revealed-secret' }],
      root: 0,
    }));

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal }));

    const { Secret } = await import('../src/agentConfig');
    const secret = new Secret(['optionalSecret'], { kind: 'string', optional: true });

    expect(secret.get()).toBe('revealed-secret');
    expect(reveal).toHaveBeenCalledOnce();
  });

  it('review repro: successful required secret config lift consumes the source owned handle node', async () => {
    const rawSecret = { id: 'opaque-secret' } as never;
    const configValue = {
      valueNodes: [{ tag: 'secret-value', val: rawSecret }],
      root: 0,
    };
    const getConfigValue = vi.fn(() => configValue);

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({
      reveal: vi.fn(() => ({
        valueNodes: [{ tag: 'string-value', val: 'revealed-secret' }],
        root: 0,
      })),
    }));

    const { Secret } = await import('../src/agentConfig');
    const secret = new Secret(['apiKey'], { kind: 'string', optional: false });

    expect(secret.get()).toBe('revealed-secret');
    expect((configValue.valueNodes[0] as { val: unknown }).val).toBeUndefined();
  });

  it('review repro: required empty list config value is not pruned as an absent optional group', async () => {
    const getConfigValue = vi.fn(() => ({
      valueNodes: [{ tag: 'list-value', val: [] }],
      root: 0,
    }));

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));

    const { Config } = await import('../src/agentConfig');
    const config = new Config(
      [
        {
          path: ['items'],
          secret: false,
          type: {
            kind: 'array',
            optional: false,
            element: { kind: 'number', optional: false },
          },
        },
      ],
      [],
    );

    expect(config.value).toEqual({ items: [] });
  });

  it('preserves an all-optional non-secret config group as an empty object', async () => {
    const getConfigValue = vi.fn(() => ({
      valueNodes: [{ tag: 'option-value', val: undefined }],
      root: 0,
    }));

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));

    const { Config } = await import('../src/agentConfig');

    const config = new Config(
      [
        {
          path: ['group', 'x'],
          secret: false,
          type: { kind: 'number', optional: true },
        },
        {
          path: ['group', 'y'],
          secret: false,
          type: { kind: 'string', optional: true },
        },
      ],
      [],
    );

    expect(config.value).toEqual({ group: { x: undefined, y: undefined } });
  });

  it('review repro: required null option config value is preserved as a present group member', async () => {
    const getConfigValue = vi.fn(() => ({
      valueNodes: [{ tag: 'option-value', val: undefined }],
      root: 0,
    }));

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));

    const { Config } = await import('../src/agentConfig');
    const { buildTypeFromJSON } = await import('@golemcloud/golem-ts-types-core');

    const config = new Config(
      [
        {
          path: ['group', 'maybeLabel'],
          secret: false,
          type: buildTypeFromJSON({
            kind: 'union',
            optional: false,
            types: [
              { kind: 'string', optional: false },
              { kind: 'null', optional: false },
            ],
            typeParams: [],
            originalTypeName: undefined,
          }),
        },
      ],
      [{ path: ['group'], requiredKeys: ['maybeLabel'] }],
    );

    expect(config.value).toEqual({ group: { maybeLabel: null } });
  });

  it('review repro: required record config leaf with only absent optional fields is preserved as present', async () => {
    const getConfigValue = vi.fn(() => ({
      valueNodes: [
        { tag: 'option-value', val: undefined },
        { tag: 'record-value', val: [0] },
      ],
      root: 1,
    }));

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));

    const { Config } = await import('../src/agentConfig');
    const { buildTypeFromJSON } = await import('@golemcloud/golem-ts-types-core');

    const config = new Config(
      [
        {
          path: ['group', 'leaf'],
          secret: false,
          type: buildTypeFromJSON({
            kind: 'interface',
            name: 'Leaf',
            owner: undefined,
            optional: false,
            typeParams: [],
            properties: [
              {
                name: 'label',
                optional: true,
                type: { kind: 'string', optional: false },
              },
            ],
          }),
        },
      ],
      [{ path: ['group'], requiredKeys: ['leaf'] }],
    );

    expect(config.value).toEqual({ group: { leaf: { label: undefined } } });
  });

  it('review repro: nested record fields inside config leaves are not pruned', async () => {
    const getConfigValue = vi.fn(() => ({
      valueNodes: [
        { tag: 'option-value', val: undefined },
        { tag: 'record-value', val: [0] },
        { tag: 'record-value', val: [1] },
      ],
      root: 2,
    }));

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));

    const { Config } = await import('../src/agentConfig');
    const { buildTypeFromJSON } = await import('@golemcloud/golem-ts-types-core');

    const config = new Config(
      [
        {
          path: ['group', 'leaf'],
          secret: false,
          type: buildTypeFromJSON({
            kind: 'interface',
            name: 'Leaf',
            owner: undefined,
            optional: false,
            typeParams: [],
            properties: [
              {
                name: 'nested',
                optional: false,
                type: {
                  kind: 'interface',
                  name: 'Nested',
                  owner: undefined,
                  optional: false,
                  typeParams: [],
                  properties: [
                    {
                      name: 'label',
                      optional: true,
                      type: { kind: 'string', optional: false },
                    },
                  ],
                },
              },
            ],
          }),
        },
      ],
      [{ path: ['group'], requiredKeys: ['leaf'] }],
    );

    expect(config.value).toEqual({ group: { leaf: { nested: { label: undefined } } } });
  });

  it('review repro: malformed non-secret config decode does not consume unexpected secret handles', async () => {
    const rawSecret = { id: 'unexpected-secret' } as never;
    const configValue = {
      valueNodes: [
        { tag: 'f64-value', val: 1 },
        { tag: 'secret-value', val: rawSecret },
        { tag: 'record-value', val: [0, 1] },
      ],
      root: 2,
    };
    const getConfigValue = vi.fn(() => configValue);

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));

    const { Config } = await import('../src/agentConfig');
    const config = new Config(
      [
        {
          path: ['value'],
          secret: false,
          type: { kind: 'number', optional: false },
        },
      ],
      [],
    );

    expect(() => config.value).toThrow(/number/);
    expect((configValue.valueNodes[1] as { val: unknown }).val).toBe(rawSecret);
  });

  it('review repro: present optional secret config reloads the config handle on each get', async () => {
    const rawSecret1 = { id: 'opaque-secret-1' } as never;
    const rawSecret2 = { id: 'opaque-secret-2' } as never;
    const getConfigValue = vi
      .fn()
      .mockReturnValueOnce({
        valueNodes: [
          { tag: 'secret-value', val: rawSecret1 },
          { tag: 'option-value', val: 0 },
        ],
        root: 1,
      })
      .mockReturnValueOnce({
        valueNodes: [
          { tag: 'secret-value', val: rawSecret1 },
          { tag: 'option-value', val: 0 },
        ],
        root: 1,
      })
      .mockReturnValueOnce({
        valueNodes: [
          { tag: 'secret-value', val: rawSecret2 },
          { tag: 'option-value', val: 0 },
        ],
        root: 1,
      })
      .mockReturnValueOnce({
        valueNodes: [
          { tag: 'secret-value', val: rawSecret2 },
          { tag: 'option-value', val: 0 },
        ],
        root: 1,
      });
    const reveal = vi.fn((raw: unknown) => ({
      valueNodes: [
        {
          tag: 'string-value',
          val: raw === rawSecret1 ? 'first-secret' : 'second-secret',
        },
      ],
      root: 0,
    }));

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal }));

    const { Config } = await import('../src/agentConfig');
    const config = new Config(
      [
        {
          path: ['optionalSecret'],
          secret: true,
          type: { kind: 'string', optional: true },
        },
      ],
      [],
    );

    expect(config.value.optionalSecret?.get()).toBe('first-secret');
    expect(config.value.optionalSecret?.get()).toBe('second-secret');
    expect(getConfigValue).toHaveBeenCalledTimes(4);
  });

  it('review repro: Config.value respects explicit optional secret handles with required payloads', async () => {
    const getConfigValue = vi.fn(() => ({
      valueNodes: [{ tag: 'option-value', val: undefined }],
      root: 0,
    }));

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({
      reveal: () => {
        throw new Error('reveal should not be called for an absent optional secret handle');
      },
    }));

    const { Config } = await import('../src/agentConfig');
    const config = new Config(
      [
        {
          path: ['apiKey'],
          secret: true,
          secretHandleOptional: true,
          type: { kind: 'string', optional: false },
        },
      ],
      [],
    );

    expect(config.value).toEqual({ apiKey: undefined });
    expect(getConfigValue).toHaveBeenCalledOnce();
  });

  it('review repro: optional secret handles preserve nullable revealed payloads', async () => {
    const rawSecret = { id: 'nullable-secret-payload' } as never;
    const getConfigValue = vi
      .fn()
      .mockReturnValueOnce({
        valueNodes: [
          { tag: 'secret-value', val: rawSecret },
          { tag: 'option-value', val: 0 },
        ],
        root: 1,
      })
      .mockReturnValueOnce({
        valueNodes: [
          { tag: 'secret-value', val: rawSecret },
          { tag: 'option-value', val: 0 },
        ],
        root: 1,
      });
    const reveal = vi.fn(() => ({
      valueNodes: [{ tag: 'option-value', val: undefined }],
      root: 0,
    }));

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal }));

    const { Config } = await import('../src/agentConfig');
    const config = new Config(
      [
        {
          path: ['apiKey'],
          secret: true,
          secretHandleOptional: true,
          type: { kind: 'string', optional: true },
        },
      ],
      [],
    );

    expect(config.value.apiKey?.get()).toBeUndefined();
    expect(reveal).toHaveBeenCalledOnce();
  });

  it('review repro: failed outbound encode does not consume a path-backed secret config handle', async () => {
    const rawSecret = { id: 'config-secret' } as never;
    const configValue = {
      valueNodes: [{ tag: 'secret-value', val: rawSecret }],
      root: 0,
    };

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({
      getConfigValue: vi.fn(() => configValue),
    }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal: vi.fn() }));

    const { Secret } = await import('../src/agentConfig');
    const { encodeInputRecordToWit } = await import('../src/internal/mapping/values/boundaryValue');
    const { r } = await import('../src/internal/mapping/types/resolvedType');

    const secretParam = {
      name: 'secret',
      type: {
        tag: 'schema',
        graph: { defs: new Map(), root: r.secret(r.string()) },
        tsType: {} as never,
      },
    };
    const badByteParam = {
      name: 'byte',
      type: {
        tag: 'schema',
        graph: { defs: new Map(), root: r.u8() },
        tsType: {} as never,
      },
    };

    const secret = new Secret(['apiKey'], { kind: 'string', optional: false });

    expect(() => encodeInputRecordToWit([secret, 256], [secretParam, badByteParam])).toThrow(
      /u8 value out of range/,
    );

    expect(configValue.valueNodes[0].val).toBe(rawSecret);
  });

  it('review repro: required secret config rejects an absent option handle', async () => {
    const getConfigValue = vi.fn(() => ({
      valueNodes: [{ tag: 'option-value', val: undefined }],
      root: 0,
    }));
    const reveal = vi.fn(() => {
      throw new Error('reveal should not be called for an absent required secret handle');
    });

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal }));

    const { Secret } = await import('../src/agentConfig');
    const secret = new Secret(['requiredSecret'], { kind: 'string', optional: false });

    expect(() => secret.get()).toThrow(/Expected secret config value|absent/);
    expect(reveal).not.toHaveBeenCalled();
  });

  it('review repro: required secret config rejects a present option-wrapped handle', async () => {
    const rawSecret = { id: 'opaque-secret' } as never;
    const getConfigValue = vi.fn(() => ({
      valueNodes: [
        { tag: 'secret-value', val: rawSecret },
        { tag: 'option-value', val: 0 },
      ],
      root: 1,
    }));
    const reveal = vi.fn(() => ({
      valueNodes: [{ tag: 'string-value', val: 'revealed-secret' }],
      root: 0,
    }));

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal }));

    const { Secret } = await import('../src/agentConfig');
    const secret = new Secret(['requiredSecret'], { kind: 'string', optional: false });

    expect(() => secret.get()).toThrow(/Expected secret config value|option/);
    expect(reveal).not.toHaveBeenCalled();
  });

  it('prunes an absent optional group whose required member is a secret', async () => {
    const getConfigValue = vi.fn(() => ({
      valueNodes: [{ tag: 'option-value', val: undefined }],
      root: 0,
    }));

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({
      reveal: () => {
        throw new Error('reveal should not be called for an absent optional secret handle');
      },
    }));

    const { Config } = await import('../src/agentConfig');

    const config = new Config(
      [
        {
          path: ['group', 'apiKey'],
          secret: true,
          type: { kind: 'string', optional: true },
        },
      ],
      [{ path: ['group'], requiredKeys: ['apiKey'] }],
    );

    expect(config.value).toEqual({ group: undefined });
    expect(getConfigValue).toHaveBeenCalledOnce();
  });

  it('review repro: prunes an absent optional group whose only members are optional secrets', async () => {
    const getConfigValue = vi.fn(() => ({
      valueNodes: [{ tag: 'option-value', val: undefined }],
      root: 0,
    }));

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({
      reveal: () => {
        throw new Error('reveal should not be called for an absent optional secret handle');
      },
    }));

    const { Config } = await import('../src/agentConfig');

    const config = new Config(
      [
        {
          path: ['group', 'maybeApiKey'],
          secret: true,
          type: { kind: 'string', optional: true },
        },
      ],
      [{ path: ['group'], requiredKeys: [] }],
    );

    expect(config.value).toEqual({ group: undefined });
    expect(getConfigValue).toHaveBeenCalledOnce();
  });

  it('review repro: prunes optional parent groups after a required nested group is removed', async () => {
    const getConfigValue = vi
      .fn()
      .mockReturnValueOnce({
        valueNodes: [{ tag: 'option-value', val: undefined }],
        root: 0,
      })
      .mockReturnValueOnce({
        valueNodes: [
          { tag: 'string-value', val: 'visible' },
          { tag: 'option-value', val: 0 },
        ],
        root: 1,
      });

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({
      reveal: () => {
        throw new Error('reveal should not be called for an absent optional secret handle');
      },
    }));

    const { Config } = await import('../src/agentConfig');

    const config = new Config(
      [
        {
          path: ['outer', 'inner', 'apiKey'],
          secret: true,
          type: { kind: 'string', optional: true },
        },
        {
          path: ['outer', 'inner', 'label'],
          secret: false,
          type: { kind: 'string', optional: true },
        },
      ],
      [
        { path: ['outer', 'inner'], requiredKeys: ['apiKey'] },
        { path: ['outer'], requiredKeys: [] },
      ],
    );

    expect(config.value).toEqual({ outer: undefined });
  });

  it('review repro: malformed revealed payload does not consume nested secret handles', async () => {
    const outerRawSecret = { id: 'outer-secret' } as never;
    const nestedRawSecret = { id: 'nested-secret' } as never;
    const revealed = {
      valueNodes: [
        { tag: 'record-value', val: [1, 2] },
        { tag: 'string-value', val: 'ok' },
        { tag: 'secret-value', val: nestedRawSecret },
      ],
      root: 0,
    };

    vi.resetModules();
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal: vi.fn(() => revealed) }));

    const { Secret } = await import('../src/agentConfig');
    const { GuestSecretHandle } = await import('../src/internal/schema-model/secretHandle');
    const { SECRET_INTERNAL } = await import('../src/internal/schema-model/secretInternal');
    const secret = Secret._fromHandle(
      SECRET_INTERNAL,
      GuestSecretHandle.fromRaw(SECRET_INTERNAL, outerRawSecret),
      {
        defs: new Map(),
        root: r.record([resolvedField('value', r.string())]),
      },
    );

    expect(() => secret.get()).toThrow(/record/);
    expect((revealed.valueNodes[2] as { val: unknown }).val).toBe(nestedRawSecret);
  });

  it('review repro: malformed secret config value does not consume nested secret handles', async () => {
    const nestedRawSecret = { id: 'nested-secret' } as never;
    const configValue = {
      valueNodes: [
        { tag: 'record-value', val: [1] },
        { tag: 'secret-value', val: nestedRawSecret },
      ],
      root: 0,
    };
    const getConfigValue = vi.fn(() => configValue);

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({
      reveal: () => {
        throw new Error('reveal should not be called for a malformed secret config handle');
      },
    }));

    const { Secret } = await import('../src/agentConfig');
    const secret = new Secret(['apiKey'], { kind: 'string', optional: false });

    expect(() => secret.get()).toThrow(/Expected secret config value/);
    expect((configValue.valueNodes[1] as { val: unknown }).val).toBe(nestedRawSecret);
  });

  it('review repro: malformed path-backed secret reveal does not consume the config handle', async () => {
    const rawSecret = { id: 'config-secret' } as never;
    const configValue = {
      valueNodes: [{ tag: 'secret-value', val: rawSecret }],
      root: 0,
    };
    const getConfigValue = vi.fn(() => configValue);
    const reveal = vi.fn(() => ({
      valueNodes: [{ tag: 'u32-value', val: 7 }],
      root: 0,
    }));

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal }));

    const { Secret } = await import('../src/agentConfig');
    const secret = new Secret(['apiKey'], { kind: 'string', optional: false });

    expect(() => secret.get()).toThrow(/string/);
    expect((configValue.valueNodes[0] as { val: unknown }).val).toBe(rawSecret);
  });

  it('drains quota-token handles when a secret config value has the wrong reachable root shape', async () => {
    const rawQuota = { id: 'wrong-root-quota-token' } as never;
    const configValue = {
      valueNodes: [{ tag: 'quota-token-handle', val: rawQuota }],
      root: 0,
    };
    const getConfigValue = vi.fn(() => configValue);

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({
      reveal: () => {
        throw new Error('reveal should not be called for a malformed secret config handle');
      },
    }));

    const { Secret } = await import('../src/agentConfig');
    const secret = new Secret(['apiKey'], { kind: 'string', optional: false });

    expect(() => secret.get()).toThrow(/Expected secret config value/);
    expect((configValue.valueNodes[0] as { val: unknown }).val).toBeUndefined();
  });
});
