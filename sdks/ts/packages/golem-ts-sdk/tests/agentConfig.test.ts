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
import { AgentClassName, AgentDecoratorOptions } from '../src';
import { TypeMetadata } from '@golemcloud/golem-ts-types-core';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';

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
        type: { kind: 'boolean', name: undefined, owner: '../src', optional: false },
      },
      {
        path: ['nested', 'nestedSecret'],
        secret: true,
        type: { kind: 'number', name: undefined, owner: '../src', optional: false },
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
    expect(configAgent.config).toEqual([
      {
        source: 'local',
        path: ['foo'],
        valueType: {
          nodes: [
            {
              name: undefined,
              owner: undefined,
              type: {
                tag: 'prim-f64-type',
              },
            },
          ],
        },
      },
      {
        source: 'local',
        path: ['bar'],
        valueType: {
          nodes: [
            {
              name: undefined,
              owner: undefined,
              type: {
                tag: 'prim-string-type',
              },
            },
          ],
        },
      },
      {
        source: 'secret',
        path: ['secret'],
        valueType: {
          nodes: [
            {
              name: undefined,
              owner: undefined,
              type: {
                tag: 'prim-bool-type',
              },
            },
          ],
        },
      },
      {
        source: 'secret',
        path: ['nested', 'nestedSecret'],
        valueType: {
          nodes: [
            {
              name: undefined,
              owner: undefined,
              type: {
                tag: 'prim-f64-type',
              },
            },
          ],
        },
      },
      {
        source: 'local',
        path: ['nested', 'a'],
        valueType: {
          nodes: [
            {
              name: undefined,
              owner: undefined,
              type: {
                tag: 'prim-bool-type',
              },
            },
          ],
        },
      },
      {
        source: 'local',
        path: ['nested', 'b'],
        valueType: {
          nodes: [
            {
              name: undefined,
              owner: undefined,
              type: {
                tag: 'list-type',
                val: 1,
              },
            },
            {
              name: undefined,
              owner: undefined,
              type: {
                tag: 'prim-f64-type',
              },
            },
          ],
        },
      },
      {
        source: 'local',
        path: ['aliasedNested', 'c'],
        valueType: {
          nodes: [
            {
              name: undefined,
              owner: undefined,
              type: {
                tag: 'prim-f64-type',
              },
            },
          ],
        },
      },
    ]);
  });

  it('config parameters should not show up in declared constructor', () => {
    const configAgent = AgentTypeRegistry.get(new AgentClassName('ConfigAgent'))!;
    const expectedSchema = {
      tag: 'tuple',
      val: [
        [
          'before',
          {
            tag: 'component-model',
            val: {
              nodes: [
                {
                  name: undefined,
                  owner: undefined,
                  type: {
                    tag: 'prim-f64-type',
                  },
                },
              ],
            },
          },
        ],
        [
          'after',
          {
            tag: 'component-model',
            val: {
              nodes: [
                {
                  name: undefined,
                  owner: undefined,
                  type: {
                    tag: 'prim-bool-type',
                  },
                },
              ],
            },
          },
        ],
      ],
    };
    expect(configAgent.constructor.inputSchema).toEqual(expectedSchema);
  });
});
