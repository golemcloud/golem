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

import { AgentType } from 'golem:agent/common';
import { AgentClassName } from '../../agentClassName';

/**
 * Singleton registry for agent types.
 */
class AgentTypeRegistryImpl {
  private readonly registry: Map<string, AgentType>;
  private readonly classNameCache: Map<string, AgentClassName>;
  private cachedAgents: AgentType[] | null = null;

  constructor() {
    this.registry = new Map();
    this.classNameCache = new Map();
  }

  register(agentClassName: AgentClassName, agentType: AgentType): void {
    const nameValue = agentClassName.value;
    this.registry.set(nameValue, agentType);
    this.classNameCache.set(nameValue, agentClassName);
    this.cachedAgents = null;
  }

  getRegisteredAgents(): AgentType[] {
    if (this.cachedAgents === null) {
      this.cachedAgents = Array.from(this.registry.values());
    }
    return this.cachedAgents;
  }

  get(agentClassName: AgentClassName): AgentType | undefined {
    return this.registry.get(agentClassName.value);
  }

  exists(agentClassName: AgentClassName): boolean {
    return this.registry.has(agentClassName.value);
  }
}

export const AgentTypeRegistry: AgentTypeRegistryImpl = new AgentTypeRegistryImpl();
