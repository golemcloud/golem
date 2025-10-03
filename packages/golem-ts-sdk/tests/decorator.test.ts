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
import * as Option from '../src/newTypes/option';
import { expect } from 'vitest';
import { BarAgentClassName, FooAgentClassName } from './testUtils';
import { AgentType, DataSchema, ElementSchema } from 'golem:agent/common';
import * as util from 'node:util';

// Test setup ensures loading agents prior to every test
// If the sample agents in the set-up changes, this test should fail
describe('Agent decorator should register the agent class and its methods into AgentTypeRegistry', () => {
  const complexAgent: AgentType = Option.getOrThrowWith(
    AgentTypeRegistry.get(BarAgentClassName),
    () => new Error('BarAgent not found in AgentTypeRegistry'),
  );

  const barAgentConstructor = complexAgent.constructor;

  if (!barAgentConstructor) {
    throw new Error('BarAgent constructor not found');
  }

  const barAgentMethod = complexAgent.methods.find(
    (method) => method.name === 'fun0',
  );

  if (!barAgentMethod) {
    throw new Error('fun0 method not found in BarAgent');
  }

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

    const elementSchema2 = getElementSchema(
      barAgentMethod.inputSchema,
      'unstructuredText',
    );

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

    const elementSchema2 = getElementSchema(
      barAgentConstructor.inputSchema,
      'unstructuredText',
    );

    const expected2 = { tag: 'unstructured-text', val: {} };

    expect(elementSchema2).toEqual(expected2);
  });

  it('should handle UnstructuredBinary in method params', () => {
    const elementSchema1 = getElementSchema(
      barAgentMethod.inputSchema,
      'unstructuredBinary',
    );

    const expected = {
      tag: 'unstructured-binary',
      val: { restrictions: [{ mimeType: 'application/json' }] },
    };

    expect(elementSchema1).toEqual(expected);
  });

  it('should handle UnstructuredBinary in constructor params', () => {
    const elementSchema1 = getElementSchema(
      barAgentConstructor.inputSchema,
      'unstructuredBinary',
    );

    const expected = {
      tag: 'unstructured-binary',
      val: { restrictions: [{ mimeType: 'application/json' }] },
    };

    expect(elementSchema1).toEqual(expected);
  });

  it('should handle `a: string | undefined` in method params', () => {
    const optionalStringInGetWeather = getWitType(
      barAgentMethod.inputSchema,
      'optionalStringType',
    );

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
    const optionalStringInGetWeather = getWitType(
      barAgentMethod.inputSchema,
      'optionalStringType',
    );

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
    const wit = getWitType(
      barAgentConstructor.inputSchema,
      'optionalUnionType',
    );

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
    const unionWithNullType = getWitType(
      barAgentMethod.inputSchema,
      'unionWithNull',
    );

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

  it('object with \`a: string | undefined\` works', () => {
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

  it('object with \`a: string | number | undefined\` works', () => {
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

  it('captures all methods and constructor with correct number of parameters', () => {
    const simpleAgent = Option.getOrThrowWith(
      AgentTypeRegistry.get(FooAgentClassName),
      () => new Error('FooAgent not found in AgentTypeRegistry'),
    );

    expect(complexAgent.methods.length).toEqual(24);
    expect(complexAgent.constructor.inputSchema.val.length).toEqual(6);
    expect(complexAgent.typeName).toEqual('my-complex-agent');
    expect(simpleAgent.methods.length).toEqual(17);
    expect(simpleAgent.constructor.inputSchema.val.length).toEqual(1);
  });

  it('should not capture overridden functions in base agents as agent methods', () => {
    const simpleAgent = Option.getOrThrowWith(
      AgentTypeRegistry.get(FooAgentClassName),
      () => new Error('FooAgent not found in AgentTypeRegistry'),
    );

    const forbidden = [
      'get-id',
      'get-agent-type',
      'load-snapshot',
      'save-snapshot',
    ];

    [complexAgent, simpleAgent].forEach((agent) => {
      forbidden.forEach((name) => {
        expect(agent.methods.find((m) => m.name === name)).toBeUndefined();
      });
    });
  });
});

function getWitType(dataSchema: DataSchema, parameterName: string) {
  const elementSchema = getElementSchema(dataSchema, parameterName);

  const witTypeOpt =
    elementSchema.tag === 'component-model' ? elementSchema.val : undefined;

  if (!witTypeOpt) {
    throw new Error(
      `Test failed - ${parameterName} is not of component-model type in getWeather function in ${BarAgentClassName.value}`,
    );
  }

  return witTypeOpt;
}

function getElementSchema(inputSchema: DataSchema, parameterName: string) {
  const schema: [string, ElementSchema] | undefined = inputSchema.val.find(
    ([name]) => name === parameterName,
  );

  if (!schema) {
    throw new Error(
      `${parameterName} not found in scheme ${util.format(inputSchema)}`,
    );
  }

  return schema[1];
}
