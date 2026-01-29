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

import { AgentInitiator } from '../agentInitiator';
import { AgentClassName } from '../../agentClassName';

/**
 * Singleton registry for agent initiators.
 *
 * Although only 1 agent instance can exist max in a container,
 * the container will end up keeping track of initiators of all agent classes
 * in the user code for obvious reasons.
 */
class AgentInitiatorRegistryImpl {
  private readonly registry: Map<string, AgentInitiator>;

  constructor() {
    this.registry = new Map();
  }

  register(agentTypeName: AgentClassName, agentInitiator: AgentInitiator): void {
    this.registry.set(agentTypeName.value, agentInitiator);
  }

  lookup(agentTypeName: string): AgentInitiator | undefined {
    return this.registry.get(agentTypeName);
  }

  entries(): IterableIterator<[string, AgentInitiator]> {
    return this.registry.entries();
  }

  agentTypeNames(): Array<string> {
    return Array.from(this.registry.keys());
  }

  exists(agentTypeName: string): boolean {
    return this.registry.has(agentTypeName);
  }
}

export const AgentInitiatorRegistry: AgentInitiatorRegistryImpl = new AgentInitiatorRegistryImpl();
