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
import { AssistantAgentClassName, WeatherAgentClassName } from './testUtils';
import { AnalysedType } from '../src/internal/mapping/types/AnalysedType';
import { AgentMethod, AgentType, DataSchema } from 'golem:agent/common';

// Test setup ensures loading agents prior to every test
// If the sample agents in the set up changes, this test should fail

test('Agent decorator should register the agent class and its methods into AgentTypeRegistry', () => {
  const assistantAgent: AgentType = Option.getOrThrowWith(
    AgentTypeRegistry.lookup(AssistantAgentClassName),
    () => new Error('AssistantAgent not found in AgentTypeRegistry'),
  );

  const weatherAgent = Option.getOrThrowWith(
    AgentTypeRegistry.lookup(WeatherAgentClassName),
    () => new Error('WeatherAgent not found in AgentTypeRegistry'),
  );

  const getWeatherAgentMethod = assistantAgent.methods.find(
    (method) => method.name === 'getWeather',
  );

  const optionalWitInMethod = getWitType(
    getWeatherAgentMethod!.inputSchema,
    'optionalStringType',
  );

  expect(optionalWitInMethod).toEqual({
    nodes: [
      { type: { tag: 'option-type', val: 1 } },
      { type: { tag: 'prim-string-type' } },
    ],
  });

  const assistantAgentConstructor = assistantAgent.constructor;

  const optionalWitInConstructor = getWitType(
    assistantAgentConstructor.inputSchema,
    'optionalStringType',
  );

  expect(optionalWitInConstructor).toEqual({
    nodes: [
      { type: { tag: 'option-type', val: 1 } },
      { type: { tag: 'prim-string-type' } },
    ],
  });

  expect(assistantAgent.methods.length).toEqual(22);
  expect(assistantAgent.constructor.inputSchema.val.length).toEqual(2);
  expect(weatherAgent.methods.length).toEqual(6);
  expect(weatherAgent.constructor.inputSchema.val.length).toEqual(1);
});

function getWitType(dataSchema: DataSchema, parameterName: string) {
  const optionalParamInput = dataSchema.val.find((s) => s[0] === parameterName);

  if (!optionalParamInput) {
    throw new Error(
      `${parameterName} not found in scheme ${JSON.stringify(dataSchema)}`,
    );
  }

  const optionalParamInputElement = optionalParamInput[1];

  const witTypeOpt =
    optionalParamInputElement.tag === 'component-model'
      ? optionalParamInputElement.val
      : undefined;

  if (!witTypeOpt) {
    throw new Error(
      `Test failed - ${parameterName} is not of component-model type in getWeather function in ${AssistantAgentClassName.value}`,
    );
  }

  return witTypeOpt;
}
