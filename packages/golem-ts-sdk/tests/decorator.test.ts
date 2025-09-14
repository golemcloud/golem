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
import { ComplexAgentClassName, SimpleAgentClassName } from './testUtils';
import { AgentType, DataSchema, WitType } from 'golem:agent/common';
import * as util from 'node:util';

// Test setup ensures loading agents prior to every test
// If the sample agents in the set-up changes, this test should fail
describe('Agent decorator should register the agent class and its methods into AgentTypeRegistry', () => {
  const complexAgent: AgentType = Option.getOrThrowWith(
    AgentTypeRegistry.lookup(ComplexAgentClassName),
    () => new Error('AssistantAgent not found in AgentTypeRegistry'),
  );

  const complexAgentConstructor = complexAgent.constructor;

  const complexAgentMethod = complexAgent.methods.find(
    (method) => method.name === 'fun0',
  );

  it('should handle `a: string | undefined` in method params', () => {
    const optionalStringInGetWeather = getWitType(
      complexAgentMethod!.inputSchema,
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
      complexAgentMethod!.inputSchema,
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

  it('should handle union with null in constructor', () => {
    const wit = getWitType(
      complexAgentConstructor.inputSchema,
      'optionalUnionType',
    );

    const expectedWit = {
      nodes: [
        { type: { tag: 'option-type', val: 1 } },
        {
          type: {
            tag: 'variant-type',
            val: [
              ['case1', 2],
              ['case2', 3],
              ['case3', 4],
              ['case4', 5],
            ],
          },
        },
        { type: { tag: 'prim-string-type' } },
        { type: { tag: 'prim-s32-type' } },
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
    const wit = getWitType(
      complexAgentMethod!.inputSchema,
      'optionalUnionType',
    );

    const expectedWit = {
      nodes: [
        { type: { tag: 'option-type', val: 1 } },
        {
          type: {
            tag: 'variant-type',
            val: [
              ['case1', 2],
              ['case2', 3],
              ['case3', 4],
              ['case4', 5],
            ],
          },
        },
        { type: { tag: 'prim-string-type' } },
        { type: { tag: 'prim-s32-type' } },
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
      complexAgentMethod!.inputSchema,
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
        { type: { tag: 'prim-s32-type' } },
      ],
    };

    expect(unionWithNullType).toEqual(expected);
  });

  it('object with \`a: string | undefined\` works', () => {
    const objectWithUnionWithNull = getWitType(
      complexAgentMethod!.inputSchema,
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
      complexAgentMethod!.inputSchema,
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
        { type: { tag: 'prim-s32-type' } },
      ],
    };

    expect(objectWithUnionWithNull2).toEqual(expected);
  });

  it('object with \`a?: string | number | undefined\` works', () => {
    const objectWithUnionWithNull2 = getWitType(
      complexAgentMethod!.inputSchema,
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
        { type: { tag: 'prim-s32-type' } },
      ],
    };

    expect(objectWithUnionWithNull2).toEqual(expected);
  });

  it('object with \`a?: string | undefined\` works', () => {
    const objectWithUnionWithNull2 = getWitType(
      complexAgentMethod!.inputSchema,
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
      AgentTypeRegistry.lookup(SimpleAgentClassName),
      () => new Error('WeatherAgent not found in AgentTypeRegistry'),
    );

    expect(complexAgent.methods.length).toEqual(22);
    expect(complexAgent.constructor.inputSchema.val.length).toEqual(3);
    expect(simpleAgent.methods.length).toEqual(9);
    expect(simpleAgent.constructor.inputSchema.val.length).toEqual(1);
  });
});

function getWitType(dataSchema: DataSchema, parameterName: string) {
  const optionalParamInput = dataSchema.val.find((s) => s[0] === parameterName);

  if (!optionalParamInput) {
    throw new Error(
      `${parameterName} not found in scheme ${util.format(dataSchema)}`,
    );
  }

  const optionalParamInputElement = optionalParamInput[1];

  const witTypeOpt =
    optionalParamInputElement.tag === 'component-model'
      ? optionalParamInputElement.val
      : undefined;

  if (!witTypeOpt) {
    throw new Error(
      `Test failed - ${parameterName} is not of component-model type in getWeather function in ${ComplexAgentClassName.value}`,
    );
  }

  return witTypeOpt;
}
