// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { describe, expect } from 'vitest';
import {
  BarAgentClassName,
  FooAgentClassName,
  EphemeralAgentClassName,
  SnapshottingDisabledAgentClassName,
  SnapshottingEnabledAgentClassName,
  SnapshottingPeriodicAgentClassName,
  SnapshottingEveryNAgentClassName,
} from './testUtils';
import { DataSchema, DataValue, ElementSchema } from 'golem:agent/common';
import * as util from 'node:util';
import { FooAgent } from './validAgents';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { toWitValue, Value } from '../src/internal/mapping/values/Value';
import { ResolvedAgent } from '../src/internal/resolvedAgent';
import { Uuid } from 'golem:agent/host';
import { AgentClassName } from '../src';

// Test setup ensures loading agents prior to every test
// If the sample agents in the set-up changes, this test should fail
describe('Agent decorator should register the agent class and its methods into AgentTypeRegistry', () => {
  const barAgent = AgentTypeRegistry.get(BarAgentClassName);

  if (!barAgent) {
    throw new Error('BarAgent not found in AgentTypeRegistry');
  }

  const barAgentConstructor = barAgent.constructor;

  if (!barAgentConstructor) {
    throw new Error('BarAgent constructor not found');
  }

  const barAgentMethod = barAgent.methods.find((method) => method.name === 'fun0');

  if (!barAgentMethod) {
    throw new Error('fun0 method not found in BarAgent');
  }

  it('should implement getAgentType properly', () => {
    const agent = new FooAgent('input');
    const agentType = agent.getAgentType();
    const knownAgentType = AgentTypeRegistry.get(FooAgentClassName);

    if (!knownAgentType) {
      throw new Error('FooAgent not found in AgentTypeRegistry');
    }

    expect(agentType).toEqual(knownAgentType);
  });

  it('should handle UnstructuredText in method params', () => {
    const elementSchema1 = getElementSchema(
      barAgentMethod.inputSchema,
      'unstructuredTextWithLanguageCode',
    );

    const expected = {
      tag: 'unstructured-text',
      val: { restrictions: [{ languageCode: 'en' }, { languageCode: 'de' }] },
    };

    expect(elementSchema1).toEqual(expected);

    const elementSchema2 = getElementSchema(barAgentMethod.inputSchema, 'unstructuredText');

    const expected2 = { tag: 'unstructured-text', val: {} };

    expect(elementSchema2).toEqual(expected2);
  });

  it('should handle UnstructuredText in constructor params', () => {
    const elementSchema1 = getElementSchema(
      barAgentConstructor.inputSchema,
      'unstructuredTextWithLanguageCode',
    );

    const expected = {
      tag: 'unstructured-text',
      val: { restrictions: [{ languageCode: 'en' }, { languageCode: 'de' }] },
    };

    expect(elementSchema1).toEqual(expected);

    const elementSchema2 = getElementSchema(barAgentConstructor.inputSchema, 'unstructuredText');

    const expected2 = { tag: 'unstructured-text', val: {} };

    expect(elementSchema2).toEqual(expected2);
  });

  it('should handle UnstructuredBinary in method params', () => {
    const elementSchema1 = getElementSchema(barAgentMethod.inputSchema, 'unstructuredBinary');

    const expected = {
      tag: 'unstructured-binary',
      val: { restrictions: [{ mimeType: 'application/json' }] },
    };

    expect(elementSchema1).toEqual(expected);
  });

  it('should handle Multimodal in method params', () => {
    const multimodalAgentMethod = barAgent.methods.find((method) => method.name === 'fun23');

    if (!multimodalAgentMethod) {
      throw new Error('fun23 method not found in BarAgent');
    }

    expect(multimodalAgentMethod.inputSchema.tag).toEqual('multimodal');

    const expected = [
      [
        'text',
        {
          tag: 'component-model',
          val: { nodes: [{ type: { tag: 'prim-string-type' } }] },
        },
      ],
      [
        'image',
        {
          tag: 'component-model',
          val: {
            nodes: [{ type: { tag: 'list-type', val: 1 } }, { type: { tag: 'prim-u8-type' } }],
          },
        },
      ],
      ['un-text', { tag: 'unstructured-text', val: {} }],
      [
        'un-binary',
        {
          tag: 'unstructured-binary',
          val: { restrictions: [{ mimeType: 'application/json' }] },
        },
      ],
    ];

    expect(multimodalAgentMethod.inputSchema.val).toEqual(expected);
  });

  it('should handle MultimodalBasic in method params', () => {
    const multimodalAgentMethod = barAgent.methods.find((method) => method.name === 'fun24');

    if (!multimodalAgentMethod) {
      throw new Error('fun24 method not found in BarAgent');
    }

    expect(multimodalAgentMethod.inputSchema.tag).toEqual('multimodal');

    const expected = [
      ['text', { tag: 'unstructured-text', val: {} }],
      [
        'binary',
        {
          tag: 'unstructured-binary',
          val: {},
        },
      ],
    ];

    expect(multimodalAgentMethod.inputSchema.val).toEqual(expected);
  });

  it('should handle UnstructuredBinary in constructor params', () => {
    const elementSchema1 = getElementSchema(barAgentConstructor.inputSchema, 'unstructuredBinary');

    const expected = {
      tag: 'unstructured-binary',
      val: { restrictions: [{ mimeType: 'application/json' }] },
    };

    expect(elementSchema1).toEqual(expected);
  });

  it('should handle `a: string | undefined` in method params', () => {
    const optionalStringInGetWeather = getWitType(barAgentMethod.inputSchema, 'optionalStringType');

    expect(optionalStringInGetWeather).toEqual({
      nodes: [
        {
          type: {
            tag: 'option-type',
            val: 1,
          },
        },
        {
          type: {
            tag: 'prim-string-type',
          },
        },
      ],
    });
  });

  it('should handle optional string in method', () => {
    const optionalStringInGetWeather = getWitType(barAgentMethod.inputSchema, 'optionalStringType');

    expect(optionalStringInGetWeather).toEqual({
      nodes: [
        {
          type: {
            tag: 'option-type',
            val: 1,
          },
        },
        {
          type: {
            tag: 'prim-string-type',
          },
        },
      ],
    });
  });

  it('should handle tagged unions in method', () => {
    const wit = getWitType(barAgentMethod.inputSchema, 'taggedUnionType');

    const expectedWit = {
      nodes: [
        {
          name: 'tagged-union',
          type: {
            tag: 'variant-type',
            val: [
              ['a', 1],
              ['b', 2],
              ['c', 3],
              ['d', 4],
              ['e', 5],
              ['f', 6],
              ['g', 7],
              ['h', 8],
              ['i', undefined],
              ['j', undefined],
            ],
          },
        },
        { type: { tag: 'prim-string-type' } },
        { type: { tag: 'prim-f64-type' } },
        { type: { tag: 'prim-bool-type' } },
        {
          name: 'union-type',
          type: {
            tag: 'variant-type',
            val: [
              ['union-type1', 1],
              ['union-type2', 2],
              ['union-type3', 5],
              ['union-type4', 3],
            ],
          },
        },
        {
          name: 'object-type',
          type: {
            tag: 'record-type',
            val: [
              ['a', 1],
              ['b', 2],
              ['c', 3],
            ],
          },
        },
        { name: 'list-type', type: { tag: 'list-type', val: 1 } },
        { name: 'tuple-type', type: { tag: 'tuple-type', val: [1, 2, 3] } },
        {
          name: 'simple-interface-type',
          type: { tag: 'record-type', val: [['n', 2]] },
        },
      ],
    };

    expect(wit).toEqual(expectedWit);
  });

  it('should handle union with only literals in method', () => {
    const wit = getWitType(barAgentMethod.inputSchema, 'unionWithOnlyLiterals');

    const expectedWit = {
      nodes: [
        {
          name: 'union-with-only-literals',
          type: { tag: 'enum-type', val: ['foo', 'bar', 'baz'] },
        },
      ],
    };

    expect(wit).toEqual(expectedWit);
  });

  it('should handle union with literals in method xxx', () => {
    const wit = getWitType(barAgentMethod.inputSchema, 'unionWithLiterals');

    const expectedWit = {
      nodes: [
        {
          name: 'union-with-literals',
          owner: undefined,
          type: {
            tag: 'variant-type',
            val: [
              ['a', undefined],
              ['b', undefined],
              ['c', undefined],
              ['union-with-literals1', 1],
            ],
          },
        },
        { type: { tag: 'record-type', val: [['n', 2]] } },
        { type: { tag: 'prim-f64-type' } },
      ],
    };

    expect(wit).toEqual(expectedWit);
  });

  it('should handle result type - exact in method', () => {
    const wit = getWitType(barAgentMethod.inputSchema, 'resultTypeExact');

    const expectedWit = {
      nodes: [
        {
          name: 'result-type-exact-both',
          type: { tag: 'result-type', val: [1, 2] },
        },
        { type: { tag: 'prim-f64-type' } },
        { type: { tag: 'prim-string-type' } },
      ],
    };

    expect(wit).toEqual(expectedWit);
  });

  it('should handle result type with different key names', () => {
    const wit = getWitType(barAgentMethod.inputSchema, 'resultTypeNonExact');

    const expectedWit = {
      nodes: [
        {
          name: 'result-type-non-exact',
          type: { tag: 'result-type', val: [1, 2] },
        },
        { type: { tag: 'prim-f64-type' } },
        { type: { tag: 'prim-string-type' } },
      ],
    };

    expect(wit).toEqual(expectedWit);
  });

  it('should handle result type with different key names for ok and err', () => {
    const wit = getWitType(barAgentMethod.inputSchema, 'resultTypeNonExact2');

    const expectedWit = {
      nodes: [
        {
          name: 'result-type-non-exact2',
          type: { tag: 'result-type', val: [1, 2] },
        },
        { type: { tag: 'prim-f64-type' } },
        { type: { tag: 'prim-string-type' } },
      ],
    };

    expect(wit).toEqual(expectedWit);
  });

  it('should handle union with null in constructor', () => {
    const wit = getWitType(barAgentConstructor.inputSchema, 'optionalUnionType');

    const expectedWit = {
      nodes: [
        { type: { tag: 'option-type', val: 1 } },
        {
          type: {
            tag: 'variant-type',
            val: [
              ['case3', 2],
              ['case4', 3],
              ['case5', 4],
              ['case6', 5],
            ],
          },
        },
        { type: { tag: 'prim-string-type' } },
        { type: { tag: 'prim-f64-type' } },
        {
          name: 'object-type',
          type: {
            tag: 'record-type',
            val: [
              ['a', 2],
              ['b', 3],
              ['c', 5],
            ],
          },
        },
        { type: { tag: 'prim-bool-type' } },
      ],
    };

    expect(wit).toEqual(expectedWit);
  });

  it('should handle optional union in method', () => {
    const wit = getWitType(barAgentMethod.inputSchema, 'optionalUnionType');

    const expectedWit = {
      nodes: [
        { type: { tag: 'option-type', val: 1 } },
        {
          type: {
            tag: 'variant-type',
            val: [
              ['case3', 2],
              ['case4', 3],
              ['case5', 4],
              ['case6', 5],
            ],
          },
        },
        { type: { tag: 'prim-string-type' } },
        { type: { tag: 'prim-f64-type' } },
        {
          name: 'object-type',
          type: {
            tag: 'record-type',
            val: [
              ['a', 2],
              ['b', 3],
              ['c', 5],
            ],
          },
        },
        { type: { tag: 'prim-bool-type' } },
      ],
    };

    expect(wit).toEqual(expectedWit);
  });

  it('union with null works', () => {
    const unionWithNullType = getWitType(barAgentMethod.inputSchema, 'unionWithNull');

    const expected = {
      nodes: [
        { type: { tag: 'option-type', val: 1 } },
        {
          type: {
            tag: 'variant-type',
            val: [
              ['case1', 2],
              ['case2', 3],
            ],
          },
        },
        { type: { tag: 'prim-string-type' } },
        { type: { tag: 'prim-f64-type' } },
      ],
    };

    expect(unionWithNullType).toEqual(expected);
  });

  it('object with a: string | undefined works', () => {
    const objectWithUnionWithNull = getWitType(
      barAgentMethod.inputSchema,
      'objectWithUnionWithUndefined1',
    );

    const expected = {
      nodes: [
        {
          name: 'object-with-union-with-undefined1',
          type: { tag: 'record-type', val: [['a', 1]] },
        },
        { type: { tag: 'option-type', val: 2 } },
        { type: { tag: 'prim-string-type' } },
      ],
    };

    expect(objectWithUnionWithNull).toEqual(expected);
  });

  it('interface with a: string | undefined works', () => {
    const witType = getWitType(barAgentMethod.inputSchema, 'interfaceWithUnionWithUndefined1');

    const expected = {
      nodes: [
        {
          name: 'interface-with-union-with-undefined1',
          type: { tag: 'record-type', val: [['a', 1]] },
        },
        { type: { tag: 'option-type', val: 2 } },
        { type: { tag: 'prim-string-type' } },
      ],
    };

    expect(witType).toEqual(expected);
  });

  it('object with a: string | number | undefined works', () => {
    const objectWithUnionWithNull2 = getWitType(
      barAgentMethod.inputSchema,
      'objectWithUnionWithUndefined2',
    );

    const expected = {
      nodes: [
        {
          name: 'object-with-union-with-undefined2',
          type: { tag: 'record-type', val: [['a', 1]] },
        },
        { type: { tag: 'option-type', val: 2 } },
        {
          type: {
            tag: 'variant-type',
            val: [
              ['case1', 3],
              ['case2', 4],
            ],
          },
        },
        { type: { tag: 'prim-string-type' } },
        { type: { tag: 'prim-f64-type' } },
      ],
    };

    expect(objectWithUnionWithNull2).toEqual(expected);
  });

  it('interface with a: string | number | undefined works', () => {
    const witType = getWitType(barAgentMethod.inputSchema, 'interfaceWithUnionWithUndefined2');

    const expected = {
      nodes: [
        {
          name: 'interface-with-union-with-undefined2',
          type: { tag: 'record-type', val: [['a', 1]] },
        },
        { type: { tag: 'option-type', val: 2 } },
        {
          type: {
            tag: 'variant-type',
            val: [
              ['case1', 3],
              ['case2', 4],
            ],
          },
        },
        { type: { tag: 'prim-string-type' } },
        { type: { tag: 'prim-f64-type' } },
      ],
    };

    expect(witType).toEqual(expected);
  });

  it('object with a?: string | number | undefined works', () => {
    const objectWithUnionWithNull2 = getWitType(
      barAgentMethod.inputSchema,
      'objectWithUnionWithUndefined3',
    );

    const expected = {
      nodes: [
        {
          name: 'object-with-union-with-undefined3',
          type: { tag: 'record-type', val: [['a', 1]] },
        },
        { type: { tag: 'option-type', val: 2 } },
        {
          type: {
            tag: 'variant-type',
            val: [
              ['case1', 3],
              ['case2', 4],
            ],
          },
        },
        { type: { tag: 'prim-string-type' } },
        { type: { tag: 'prim-f64-type' } },
      ],
    };

    expect(objectWithUnionWithNull2).toEqual(expected);
  });

  it('interface with a?: string | number | undefined works', () => {
    const witType = getWitType(barAgentMethod.inputSchema, 'interfaceWithUnionWithUndefined3');

    const expected = {
      nodes: [
        {
          name: 'interface-with-union-with-undefined3',
          type: { tag: 'record-type', val: [['a', 1]] },
        },
        { type: { tag: 'option-type', val: 2 } },
        {
          type: {
            tag: 'variant-type',
            val: [
              ['case1', 3],
              ['case2', 4],
            ],
          },
        },
        { type: { tag: 'prim-string-type' } },
        { type: { tag: 'prim-f64-type' } },
      ],
    };

    expect(witType).toEqual(expected);
  });

  it('object with `a?: string | undefined` works', () => {
    const objectWithUnionWithNull2 = getWitType(
      barAgentMethod.inputSchema,
      'objectWithUnionWithUndefined4',
    );

    const expected = {
      nodes: [
        {
          name: 'object-with-union-with-undefined4',
          type: { tag: 'record-type', val: [['a', 1]] },
        },
        { type: { tag: 'option-type', val: 2 } },
        { type: { tag: 'prim-string-type' } },
      ],
    };

    expect(objectWithUnionWithNull2).toEqual(expected);
  });

  it('interface with `a?: string | undefined` works', () => {
    const witType = getWitType(barAgentMethod.inputSchema, 'interfaceWithUnionWithUndefined4');

    const expected = {
      nodes: [
        {
          name: 'interface-with-union-with-undefined4',
          type: { tag: 'record-type', val: [['a', 1]] },
        },
        { type: { tag: 'option-type', val: 2 } },
        { type: { tag: 'prim-string-type' } },
      ],
    };

    expect(witType).toEqual(expected);
  });

  it('object with optional prop works', () => {
    const objectWithUnionWithNull2 = getWitType(barAgentMethod.inputSchema, 'objectWithOption');

    const expected = {
      nodes: [
        {
          name: 'object-with-option',
          type: { tag: 'record-type', val: [['a', 1]] },
        },
        { type: { tag: 'option-type', val: 2 } },
        { type: { tag: 'prim-string-type' } },
      ],
    };

    expect(objectWithUnionWithNull2).toEqual(expected);
  });

  it('interface with optional prop works', () => {
    const witType = getWitType(barAgentMethod.inputSchema, 'interfaceWithOption');

    const expected = {
      nodes: [
        {
          name: 'interface-with-option',
          type: { tag: 'record-type', val: [['a', 1]] },
        },
        { type: { tag: 'option-type', val: 2 } },
        { type: { tag: 'prim-string-type' } },
      ],
    };

    expect(witType).toEqual(expected);
  });

  it('captures all methods and constructor with correct number of parameters', () => {
    const simpleAgent = AgentTypeRegistry.get(FooAgentClassName);

    if (!simpleAgent) {
      throw new Error('FooAgent not found in AgentTypeRegistry');
    }

    // Ensures no functions or constructors are skipped
    expect(barAgent.methods.length).toEqual(25);
    expect(barAgent.constructor.inputSchema.val.length).toEqual(6);
    expect(barAgent.typeName).toEqual('my-complex-agent');
    expect(simpleAgent.methods.length).toEqual(45);
    expect(simpleAgent.constructor.inputSchema.val.length).toEqual(1);
  });

  it('should not capture overridden functions in base agents as agent methods', () => {
    const simpleAgent = AgentTypeRegistry.get(FooAgentClassName);

    if (!simpleAgent) {
      throw new Error('FooAgent not found in AgentTypeRegistry');
    }

    const forbidden = ['get-id', 'get-agent-type', 'load-snapshot', 'save-snapshot'];

    [barAgent, simpleAgent].forEach((agent) => {
      forbidden.forEach((name) => {
        expect(agent.methods.find((m) => m.name === name)).toBeUndefined();
      });
    });
  });

  it('should set durability mode to ephemeral in the registered AgentType when set in decorator options', () => {
    const ephemeralAgent = AgentTypeRegistry.get(EphemeralAgentClassName);

    if (!ephemeralAgent) {
      throw new Error('EphemeralAgent not found in AgentTypeRegistry');
    }

    expect(ephemeralAgent.mode).toEqual('ephemeral');
  });

  it('should set snapshotting to disabled by default when not specified', () => {
    const fooAgent = AgentTypeRegistry.get(FooAgentClassName);
    if (!fooAgent) throw new Error('FooAgent not found');
    expect(fooAgent.snapshotting).toEqual({ tag: 'disabled' });
  });

  it('should set snapshotting to disabled when explicitly set', () => {
    const agent = AgentTypeRegistry.get(SnapshottingDisabledAgentClassName);
    if (!agent) throw new Error('SnapshottingDisabledAgent not found');
    expect(agent.snapshotting).toEqual({ tag: 'disabled' });
  });

  it('should set snapshotting to enabled with default config', () => {
    const agent = AgentTypeRegistry.get(SnapshottingEnabledAgentClassName);
    if (!agent) throw new Error('SnapshottingEnabledAgent not found');
    expect(agent.snapshotting).toEqual({ tag: 'enabled', val: { tag: 'default' } });
  });

  it('should set snapshotting to periodic', () => {
    const agent = AgentTypeRegistry.get(SnapshottingPeriodicAgentClassName);
    if (!agent) throw new Error('SnapshottingPeriodicAgent not found');
    expect(agent.snapshotting).toEqual({
      tag: 'enabled',
      val: { tag: 'periodic', val: 5000000000n },
    });
  });

  it('should set snapshotting to every-n-invocation', () => {
    const agent = AgentTypeRegistry.get(SnapshottingEveryNAgentClassName);
    if (!agent) throw new Error('SnapshottingEveryNAgent not found');
    expect(agent.snapshotting).toEqual({
      tag: 'enabled',
      val: { tag: 'every-n-invocation', val: 10 },
    });
  });
});

describe('Annotated FooAgent class', () => {
  it('has get method', () => {
    expect(FooAgent.get).toBeDefined();
    expect(FooAgent.get).toBeTypeOf('function');
  });
  it('has phantom methods', () => {
    expect(FooAgent.getPhantom).toBeDefined();
    expect(FooAgent.getPhantom).toBeTypeOf('function');
    expect(FooAgent.newPhantom).toBeDefined();
    expect(FooAgent.newPhantom).toBeTypeOf('function');
  });
  it("can return it's phantomId", () => {
    const initiator = AgentInitiatorRegistry.lookup('FooAgent');

    if (!initiator) {
      throw new Error('FooAgent not found in AgentInitiatorRegistry');
    }

    const value: Value = { kind: 'string', value: 'hello' };

    const uuid: Uuid = {
      highBits: BigInt(1234),
      lowBits: BigInt(5678),
    };

    (globalThis as unknown as { currentAgentId: string }).currentAgentId =
      `foo-agent("hello")[${uuid.highBits}-${uuid.lowBits}]`;

    const fooResult = initiator.initiate(
      {
        tag: 'tuple',
        val: [{ tag: 'component-model', val: toWitValue(value) }],
      },
      { tag: 'anonymous' },
    );
    expect(fooResult.tag).toEqual('ok');
    const foo = fooResult.val as ResolvedAgent;
    expect(foo.phantomId()).toEqual(uuid);
  });
  it('get is implemented by the decorator', async () => {
    const agentType = AgentTypeRegistry.get(new AgentClassName('FooAgent'));

    if (!agentType) {
      throw new Error('FooAgent not found in AgentTypeRegistry');
    }

    const client = FooAgent.get('example');
    expect(client).toBeDefined();

    // NOTE: this agent id is not a valid agent-id syntax, just the one the mocked makeAgentId produces.
    expect((await client.getId()).value).toEqual(
      'FooAgent({"tag":"tuple","val":[{"tag":"component-model","val":{"nodes":[{"tag":"prim-string","val":"example"}]}}]})',
    );
    expect(await client.phantomId()).toBeUndefined();
    expect(await client.getAgentType()).toEqual(agentType);
  });
});

describe('Agent with principal auto injected', async () => {
  await import('./agentsWithPrincipalAutoInjection');

  it("should never include anything about principal in the agent's constructor or method schemas", () => {
    const agentType = AgentTypeRegistry.get(new AgentClassName('AgentWithPrincipalAutoInjection1'));

    if (!agentType) {
      throw new Error('AgentWithPrincipalAutoInjection not found in AgentTypeRegistry');
    }

    const constructorParamNames = agentType.constructor.inputSchema.val.map(([name]) => name);

    expect(constructorParamNames).not.toContain('principal');

    agentType.methods.forEach((method) => {
      const methodParamNames = method.inputSchema.val.map(([name]) => name);
      expect(methodParamNames).not.toContain('principal');
    });
  });
});

describe('Annotated SingletonAgent class', () => {
  it('can be constructed', async () => {
    const initiator = AgentInitiatorRegistry.lookup('SingletonAgent');

    if (!initiator) {
      throw new Error('SingletonAgent not found in AgentInitiatorRegistry');
    }

    const params: DataValue = {
      tag: 'tuple',
      val: [],
    };

    (globalThis as unknown as { currentAgentId: string }).currentAgentId =
      `singleton-agent(${JSON.stringify(params)})`;

    const singleton = initiator.initiate(params, { tag: 'anonymous' });
    expect(singleton.tag).toEqual('ok');
    const foo = singleton.val as ResolvedAgent;
    expect(foo.phantomId()).toBeUndefined();

    const result = await foo.invoke(
      'test',
      {
        tag: 'tuple',
        val: [],
      },
      { tag: 'anonymous' },
    );

    expect(result.tag).toEqual('ok');
    expect(result.val).toEqual({
      tag: 'tuple',
      val: [
        {
          tag: 'component-model',
          val: {
            nodes: [
              {
                tag: 'prim-string',
                val: 'test',
              },
            ],
          },
        },
      ],
    });
  });
});

function getWitType(dataSchema: DataSchema, parameterName: string) {
  const elementSchema = getElementSchema(dataSchema, parameterName);

  const witType = elementSchema.tag === 'component-model' ? elementSchema.val : undefined;

  if (!witType) {
    throw new Error(
      `Test failed - ${parameterName} is not of component-model type in getWeather function in ${BarAgentClassName.value}`,
    );
  }

  return witType;
}

function getElementSchema(inputSchema: DataSchema, parameterName: string) {
  const schema: [string, ElementSchema] | undefined = inputSchema.val.find(
    ([name]) => name === parameterName,
  );

  if (!schema) {
    throw new Error(`${parameterName} not found in scheme ${util.format(inputSchema)}`);
  }

  return schema[1];
}
