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

import { describe, it } from 'vitest';
import { AgentClassName } from '../src';
import { TypeMetadata } from '@golemcloud/golem-ts-types-core';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { paramNames, paramShape, schemaTypeAt, normalizeSchema } from './agentTypeHelpers';

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
        type: { kind: 'number', name: undefined, owner: undefined, optional: false },
      },
      {
        path: ['bar'],
        secret: false,
        type: { kind: 'string', name: undefined, owner: undefined, optional: false },
      },
      {
        path: ['secret'],
        secret: true,
        type: { kind: 'boolean', name: undefined, owner: undefined, optional: false },
      },
      {
        path: ['nested', 'nestedSecret'],
        secret: true,
        type: { kind: 'number', name: undefined, owner: undefined, optional: false },
      },
      {
        path: ['nested', 'a'],
        secret: false,
        type: { kind: 'boolean', name: undefined, owner: undefined, optional: false },
      },
      {
        path: ['nested', 'b'],
        secret: false,
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
      { source: 'secret', path: ['secret'], type: 'bool' },
      { source: 'secret', path: ['nested', 'nestedSecret'], type: 'f64' },
      { source: 'local', path: ['nested', 'a'], type: 'bool' },
      { source: 'local', path: ['nested', 'b'], type: { list: 'f64' } },
      { source: 'local', path: ['aliasedNested', 'c'], type: 'f64' },
    ]);
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
