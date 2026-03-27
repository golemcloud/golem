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

import { test, expect } from 'vitest';
import { clearAgentValidationError } from '../src';
import { getAgentValidationError } from '../src/decorators/agent';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { AgentClassName } from '../src/agentClassName';

test('HTTP validation errors are stored during agent registration, not thrown', async () => {
  // Clear any previous state
  clearAgentValidationError();

  // Dynamically import an agent with invalid HTTP configuration
  // This should NOT throw an error during import
  await expect(import('./agentWithInvalidHttpMount1')).resolves.toBeDefined();

  // The validation error should be stored
  const error = getAgentValidationError();
  expect(error).toBeDefined();
  expect(error?.message).toContain("HTTP validation failed for agent 'AgentWithInvalidHttpMount1'");
  expect(error?.message).toContain(
    "Agent constructor variable 'bar' is not provided by the HTTP mount path",
  );

  // The agent should still be registered in the registry
  const agentType = AgentTypeRegistry.get(new AgentClassName('AgentWithInvalidHttpMount1'));
  expect(agentType).toBeDefined();
  expect(agentType?.typeName).toBe('AgentWithInvalidHttpMount1');

  // Clean up
  clearAgentValidationError();
});
