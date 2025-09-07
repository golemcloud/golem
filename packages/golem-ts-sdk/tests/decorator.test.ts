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

// Test setup ensures loading agents prior to every test
// If the sample agents in the set up changes, this test should fail
test('Agent decorator should register the agent class and its methods into AgentTypeRegistry', () => {
  const assistantAgent = Option.getOrThrowWith(
    AgentTypeRegistry.lookup(AssistantAgentClassName),
    () => new Error('AssistantAgent not found in AgentTypeRegistry'),
  );

  const weatherAgent = Option.getOrThrowWith(
    AgentTypeRegistry.lookup(WeatherAgentClassName),
    () => new Error('WeatherAgent not found in AgentTypeRegistry'),
  );

  expect(assistantAgent.methods.length).toEqual(1);
  expect(assistantAgent.constructor.inputSchema.val.length).toEqual(1);
  expect(weatherAgent.methods.length).toEqual(3);
  expect(weatherAgent.constructor.inputSchema.val.length).toEqual(1);
});
