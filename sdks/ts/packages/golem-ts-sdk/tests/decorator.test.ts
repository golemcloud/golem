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
  ConstructorUnionOrderAgentClassName,
  ReadOnlyAgentClassName,
} from './testUtils';
import { InputSchema } from 'golem:agent/common@2.0.0';
import { EphemeralAgent, FooAgent } from './validAgents';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { ResolvedAgent } from '../src/internal/resolvedAgent';
import { Uuid } from '../src/uuid';
import { AgentClassName, BaseAgent, ParsedAgentId } from '../src';
import { v, schemaValueFromWit, schemaValueToWit } from '../src/internal/schema-model';
import { paramNames, paramRoleTag, paramShape } from './agentTypeHelpers';

// Shared structural shapes (in the compact `normalizeSchema` form produced by
// `paramShape`). Named TypeScript types project to `ref`s into the agent's
// schema graph and surface here as `{ def: <name>, ... }`.
const objectTypeShape = {
  def: 'ObjectType',
  record: [
    ['a', 'string'],
    ['b', 'f64'],
    ['c', 'bool'],
  ],
};

const unionTypeShape = {
  def: 'UnionType',
  variant: [
    ['UnionType1', 'f64'],
    ['UnionType2', 'string'],
    ['UnionType3', 'bool'],
    ['UnionType4', objectTypeShape],
  ],
};

const simpleInterfaceShape = { def: 'SimpleInterfaceType', record: [['n', 'f64']] };

const unstructuredTextShape = {
  variant: [
    ['inline', { text: {} }],
    ['url', { url: {} }],
  ],
};
const unstructuredTextLangShape = {
  variant: [
    ['inline', { text: { languages: ['en', 'de'] } }],
    ['url', { url: {} }],
  ],
};
const unstructuredBinaryJsonShape = {
  variant: [
    ['inline', { binary: { mimeTypes: ['application/json'] } }],
    ['url', { url: {} }],
  ],
};
const unstructuredBinaryAnyShape = {
  variant: [
    ['inline', { binary: {} }],
    ['url', { url: {} }],
  ],
};

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

  const constructorUnionOrderAgent = AgentTypeRegistry.get(ConstructorUnionOrderAgentClassName);

  if (!constructorUnionOrderAgent) {
    throw new Error('ConstructorUnionOrderAgent not found in AgentTypeRegistry');
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
    expect(
      paramShape(barAgent, barAgentMethod.inputSchema, 'unstructuredTextWithLanguageCode'),
    ).toEqual(unstructuredTextLangShape);

    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'unstructuredText')).toEqual(
      unstructuredTextShape,
    );

    // The unstructured-text role marks the variant root node itself.
    expect(
      paramRoleTag(barAgent, barAgentMethod.inputSchema, 'unstructuredTextWithLanguageCode'),
    ).toEqual('unstructured-text');
    expect(paramRoleTag(barAgent, barAgentMethod.inputSchema, 'unstructuredText')).toEqual(
      'unstructured-text',
    );
  });

  it('should handle UnstructuredText in constructor params', () => {
    expect(
      paramShape(barAgent, barAgentConstructor.inputSchema, 'unstructuredTextWithLanguageCode'),
    ).toEqual(unstructuredTextLangShape);

    expect(paramShape(barAgent, barAgentConstructor.inputSchema, 'unstructuredText')).toEqual(
      unstructuredTextShape,
    );
  });

  it('should handle UnstructuredBinary in method params', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'unstructuredBinary')).toEqual(
      unstructuredBinaryJsonShape,
    );

    // The unstructured-binary role marks the variant root node itself.
    expect(paramRoleTag(barAgent, barAgentMethod.inputSchema, 'unstructuredBinary')).toEqual(
      'unstructured-binary',
    );
  });

  it('should handle Multimodal in method params', () => {
    const multimodalAgentMethod = barAgent.methods.find((method) => method.name === 'fun23');

    if (!multimodalAgentMethod) {
      throw new Error('fun23 method not found in BarAgent');
    }

    // A multimodal parameter is a single record field carrying a `list<variant>`.
    expect(multimodalAgentMethod.inputSchema.tag).toEqual('parameters');
    expect(paramNames(multimodalAgentMethod.inputSchema)).toEqual(['multimodalInput']);

    expect(paramShape(barAgent, multimodalAgentMethod.inputSchema, 'multimodalInput')).toEqual({
      list: {
        variant: [
          ['text', 'string'],
          ['image', { list: 'u8' }],
          ['un-text', unstructuredTextShape],
          ['un-binary', unstructuredBinaryJsonShape],
        ],
      },
    });
    // The multimodal role must live on the `list` (root) node itself.
    expect(paramRoleTag(barAgent, multimodalAgentMethod.inputSchema, 'multimodalInput')).toEqual(
      'multimodal',
    );
  });

  it('should handle MultimodalBasic in method params', () => {
    const multimodalAgentMethod = barAgent.methods.find((method) => method.name === 'fun24');

    if (!multimodalAgentMethod) {
      throw new Error('fun24 method not found in BarAgent');
    }

    expect(multimodalAgentMethod.inputSchema.tag).toEqual('parameters');
    expect(paramNames(multimodalAgentMethod.inputSchema)).toEqual(['multimodalInput']);

    expect(paramShape(barAgent, multimodalAgentMethod.inputSchema, 'multimodalInput')).toEqual({
      list: {
        variant: [
          ['text', unstructuredTextShape],
          ['binary', unstructuredBinaryAnyShape],
        ],
      },
    });
    expect(paramRoleTag(barAgent, multimodalAgentMethod.inputSchema, 'multimodalInput')).toEqual(
      'multimodal',
    );
  });

  it('should handle UnstructuredBinary in constructor params', () => {
    expect(paramShape(barAgent, barAgentConstructor.inputSchema, 'unstructuredBinary')).toEqual(
      unstructuredBinaryJsonShape,
    );
  });

  it('should handle `a: string | undefined` in method params', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'optionalStringType')).toEqual({
      option: 'string',
    });
  });

  it('should handle optional string in method', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'optionalStringType')).toEqual({
      option: 'string',
    });
  });

  it('should handle boolean|undefined as option<bool> in method', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'optionalBooleanType')).toEqual({
      option: 'bool',
    });
  });

  it('should handle tagged unions in method', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'taggedUnionType')).toEqual({
      def: 'TaggedUnion',
      variant: [
        ['a', 'string'],
        ['b', 'f64'],
        ['c', 'bool'],
        ['d', unionTypeShape],
        ['e', objectTypeShape],
        ['f', { list: 'string' }],
        ['g', { tuple: ['string', 'f64', 'bool'] }],
        ['h', simpleInterfaceShape],
        ['i', null],
        ['j', null],
      ],
    });
  });

  it('should handle union with only literals in method', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'unionWithOnlyLiterals')).toEqual({
      def: 'UnionWithOnlyLiterals',
      enum: ['foo', 'bar', 'baz'],
    });
  });

  it('should handle union with literals in method xxx', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'unionWithLiterals')).toEqual({
      def: 'UnionWithLiterals',
      variant: [
        ['a', null],
        ['b', null],
        ['c', null],
        ['UnionWithLiterals1', { record: [['n', 'f64']] }],
      ],
    });
  });

  it('should handle result type - exact in method', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'resultTypeExact')).toEqual({
      result: ['f64', 'string'],
    });
  });

  it('should handle result type with different key names', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'resultTypeNonExact')).toEqual({
      result: ['f64', 'string'],
    });
  });

  it('should handle result type with different key names for ok and err', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'resultTypeNonExact2')).toEqual({
      result: ['f64', 'string'],
    });
  });

  it('should handle union with null in constructor', () => {
    expect(paramShape(barAgent, barAgentConstructor.inputSchema, 'optionalUnionType')).toEqual({
      option: unionTypeShape,
    });
  });

  it('should handle optional union in method', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'optionalUnionType')).toEqual({
      option: unionTypeShape,
    });
  });

  it('should handle object|boolean|undefined in method', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'objectOrBooleanOrUndefined')).toEqual({
      option: {
        def: 'ObjectOrBooleanOrUndefined',
        variant: [
          [
            'ObjectOrBooleanOrUndefined1',
            {
              record: [
                ['a', 'f64'],
                ['b', 'string'],
              ],
            },
          ],
          ['ObjectOrBooleanOrUndefined2', 'bool'],
        ],
      },
    });
  });

  it('should preserve constructor union order for inline object|boolean|undefined', () => {
    expect(
      paramShape(
        constructorUnionOrderAgent,
        constructorUnionOrderAgent.constructor.inputSchema,
        'complex',
      ),
    ).toEqual({
      option: {
        variant: [
          [
            'case1',
            {
              record: [
                ['a', 'f64'],
                ['b', 'string'],
              ],
            },
          ],
          ['case2', 'bool'],
        ],
      },
    });
  });

  it('should handle boolean|undefined as option<bool> in constructor', () => {
    expect(
      paramShape(
        constructorUnionOrderAgent,
        constructorUnionOrderAgent.constructor.inputSchema,
        'flag',
      ),
    ).toEqual({ option: 'bool' });
  });

  it('union with null works', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'unionWithNull')).toEqual({
      option: {
        variant: [
          ['case1', 'string'],
          ['case2', 'f64'],
        ],
      },
    });
  });

  it('object with a: string | undefined works', () => {
    expect(
      paramShape(barAgent, barAgentMethod.inputSchema, 'objectWithUnionWithUndefined1'),
    ).toEqual({
      def: 'ObjectWithUnionWithUndefined1',
      record: [['a', { option: 'string' }]],
    });
  });

  it('interface with a: string | undefined works', () => {
    expect(
      paramShape(barAgent, barAgentMethod.inputSchema, 'interfaceWithUnionWithUndefined1'),
    ).toEqual({
      def: 'InterfaceWithUnionWithUndefined1',
      record: [['a', { option: 'string' }]],
    });
  });

  it('object with a: string | number | undefined works', () => {
    expect(
      paramShape(barAgent, barAgentMethod.inputSchema, 'objectWithUnionWithUndefined2'),
    ).toEqual({
      def: 'ObjectWithUnionWithUndefined2',
      record: [
        [
          'a',
          {
            option: {
              variant: [
                ['case1', 'string'],
                ['case2', 'f64'],
              ],
            },
          },
        ],
      ],
    });
  });

  it('interface with a: string | number | undefined works', () => {
    expect(
      paramShape(barAgent, barAgentMethod.inputSchema, 'interfaceWithUnionWithUndefined2'),
    ).toEqual({
      def: 'InterfaceWithUnionWithUndefined2',
      record: [
        [
          'a',
          {
            option: {
              variant: [
                ['case1', 'string'],
                ['case2', 'f64'],
              ],
            },
          },
        ],
      ],
    });
  });

  it('object with a?: string | number | undefined works', () => {
    expect(
      paramShape(barAgent, barAgentMethod.inputSchema, 'objectWithUnionWithUndefined3'),
    ).toEqual({
      def: 'ObjectWithUnionWithUndefined3',
      record: [
        [
          'a',
          {
            option: {
              variant: [
                ['case1', 'string'],
                ['case2', 'f64'],
              ],
            },
          },
        ],
      ],
    });
  });

  it('interface with a?: string | number | undefined works', () => {
    expect(
      paramShape(barAgent, barAgentMethod.inputSchema, 'interfaceWithUnionWithUndefined3'),
    ).toEqual({
      def: 'InterfaceWithUnionWithUndefined3',
      record: [
        [
          'a',
          {
            option: {
              variant: [
                ['case1', 'string'],
                ['case2', 'f64'],
              ],
            },
          },
        ],
      ],
    });
  });

  it('object with `a?: string | undefined` works', () => {
    expect(
      paramShape(barAgent, barAgentMethod.inputSchema, 'objectWithUnionWithUndefined4'),
    ).toEqual({
      def: 'ObjectWithUnionWithUndefined4',
      record: [['a', { option: 'string' }]],
    });
  });

  it('interface with `a?: string | undefined` works', () => {
    expect(
      paramShape(barAgent, barAgentMethod.inputSchema, 'interfaceWithUnionWithUndefined4'),
    ).toEqual({
      def: 'InterfaceWithUnionWithUndefined4',
      record: [['a', { option: 'string' }]],
    });
  });

  it('object with optional prop works', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'objectWithOption')).toEqual({
      def: 'ObjectWithOption',
      record: [['a', { option: 'string' }]],
    });
  });

  it('interface with optional prop works', () => {
    expect(paramShape(barAgent, barAgentMethod.inputSchema, 'interfaceWithOption')).toEqual({
      def: 'InterfaceWithOption',
      record: [['a', { option: 'string' }]],
    });
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
    expect(simpleAgent.methods.length).toEqual(46);
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

  it('should expose read-only configuration with default cache policy', () => {
    const agent = AgentTypeRegistry.get(ReadOnlyAgentClassName);
    if (!agent) throw new Error('ReadOnlyAgent not found');
    const method = agent.methods.find((m) => m.name === 'defaultCache');
    expect(method).toBeDefined();
    expect(method!.readOnly).toEqual({
      cachePolicy: { tag: 'until-write' },
      usesPrincipal: false,
    });
  });

  it('should expose read-only configuration with no-cache policy', () => {
    const agent = AgentTypeRegistry.get(ReadOnlyAgentClassName);
    if (!agent) throw new Error('ReadOnlyAgent not found');
    const method = agent.methods.find((m) => m.name === 'noCache');
    expect(method!.readOnly).toEqual({
      cachePolicy: { tag: 'no-cache' },
      usesPrincipal: false,
    });
  });

  it('should expose read-only configuration with until-write policy', () => {
    const agent = AgentTypeRegistry.get(ReadOnlyAgentClassName);
    if (!agent) throw new Error('ReadOnlyAgent not found');
    const method = agent.methods.find((m) => m.name === 'untilWrite');
    expect(method!.readOnly).toEqual({
      cachePolicy: { tag: 'until-write' },
      usesPrincipal: false,
    });
  });

  it('should expose read-only configuration with ttl policy', () => {
    const agent = AgentTypeRegistry.get(ReadOnlyAgentClassName);
    if (!agent) throw new Error('ReadOnlyAgent not found');
    const method = agent.methods.find((m) => m.name === 'ttl');
    expect(method!.readOnly).toEqual({
      cachePolicy: { tag: 'ttl', val: 30_000_000_000n },
      usesPrincipal: false,
    });
  });

  it('should derive usesPrincipal=true when the method has a Principal parameter', () => {
    const agent = AgentTypeRegistry.get(ReadOnlyAgentClassName);
    if (!agent) throw new Error('ReadOnlyAgent not found');
    const method = agent.methods.find((m) => m.name === 'withPrincipal');
    expect(method!.readOnly).toEqual({
      cachePolicy: { tag: 'until-write' },
      usesPrincipal: true,
    });
  });

  it('should not set read-only for unannotated methods', () => {
    const agent = AgentTypeRegistry.get(ReadOnlyAgentClassName);
    if (!agent) throw new Error('ReadOnlyAgent not found');
    const method = agent.methods.find((m) => m.name === 'notReadOnly');
    expect(method!.readOnly).toBeUndefined();
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

    const uuid = new Uuid(BigInt(1234), BigInt(5678));

    // The agent id (with embedded phantom id) is produced through the same
    // schema-native `make-agent-id` boundary the runtime uses.
    (globalThis as any).currentAgentId = ParsedAgentId.make(
      'FooAgent',
      v.record([v.string('hello')]),
      { highBits: uuid.highBits, lowBits: uuid.lowBits },
    ).value;

    const fooResult = initiator.initiate(v.record([v.string('hello')]), { tag: 'anonymous' });
    expect(fooResult.tag).toEqual('ok');
    const foo = fooResult.val as ResolvedAgent;
    const phantomId = foo.phantomId();
    expect(phantomId).toBeInstanceOf(Uuid);
    expect(phantomId!.highBits).toEqual(uuid.highBits);
    expect(phantomId!.lowBits).toEqual(uuid.lowBits);
  });
  it('get is implemented by the decorator', async () => {
    const agentType = AgentTypeRegistry.get(new AgentClassName('FooAgent'));

    if (!agentType) {
      throw new Error('FooAgent not found in AgentTypeRegistry');
    }

    const client = FooAgent.get('example');
    expect(client).toBeDefined();

    // The constructor parameters cross the boundary as a schema-value-tree
    // record; the mocked makeAgentId embeds its JSON form in the agent id.
    const expectedTree = schemaValueToWit(v.record([v.string('example')]));
    expect((await client.getId()).value).toEqual(`FooAgent(${JSON.stringify(expectedTree)})`);
    expect(await client.phantomId()).toBeUndefined();
    expect(await client.getAgentType()).toEqual(agentType);
  });
});

describe('Annotated EphemeralAgent class', () => {
  it('does not override non-phantom constructors', () => {
    expect(EphemeralAgent.get).toBe(BaseAgent.get);
    expect(EphemeralAgent.getWithConfig).toBe(BaseAgent.getWithConfig);
  });

  it('still exposes phantom constructors', () => {
    expect(EphemeralAgent.getPhantom).toBeDefined();
    expect(EphemeralAgent.getPhantom).toBeTypeOf('function');
    expect(EphemeralAgent.newPhantom).toBeDefined();
    expect(EphemeralAgent.newPhantom).toBeTypeOf('function');
  });
});

describe('Agent with principal auto injected', async () => {
  await import('./agentsWithPrincipalAutoInjection');

  it('exposes principal only as an auto-injected field, never as a user-supplied parameter', () => {
    const agentType = AgentTypeRegistry.get(new AgentClassName('AgentWithPrincipalAutoInjection1'));

    if (!agentType) {
      throw new Error('AgentWithPrincipalAutoInjection not found in AgentTypeRegistry');
    }

    const userSupplied = (schema: InputSchema) =>
      schema.val.filter((f) => f.source.tag === 'user-supplied').map((f) => f.name);
    const autoInjectedPrincipals = (schema: InputSchema) =>
      schema.val.filter((f) => f.source.tag === 'auto-injected' && f.source.val === 'principal');

    // Principal is never part of the user-supplied value contract...
    expect(userSupplied(agentType.constructor.inputSchema)).not.toContain('principal');
    // ...but it IS declared as an auto-injected field so the host injects it.
    expect(autoInjectedPrincipals(agentType.constructor.inputSchema).length).toBeGreaterThan(0);

    agentType.methods.forEach((method) => {
      expect(userSupplied(method.inputSchema)).not.toContain('principal');
    });
  });
});

describe('Annotated SingletonAgent class', () => {
  it('can be constructed', async () => {
    const initiator = AgentInitiatorRegistry.lookup('SingletonAgent');

    if (!initiator) {
      throw new Error('SingletonAgent not found in AgentInitiatorRegistry');
    }

    // No constructor parameters -> an empty input record.
    const params = v.record([]);
    (globalThis as any).currentAgentId = ParsedAgentId.make('SingletonAgent', params).value;

    const singleton = initiator.initiate(params, { tag: 'anonymous' });
    expect(singleton.tag).toEqual('ok');
    const foo = singleton.val as ResolvedAgent;
    expect(foo.phantomId()).toBeUndefined();

    const result = await foo.invoke('test', schemaValueToWit(v.record([])), { tag: 'anonymous' });

    if (result.tag !== 'ok') {
      throw new Error(`Invocation failed: ${JSON.stringify(result.val)}`);
    }
    expect(result.val !== undefined ? schemaValueFromWit(result.val) : undefined).toEqual({
      tag: 'string',
      value: 'test',
    });
  });
});
