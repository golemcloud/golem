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

interface AgentConstructorMetadata {
  prompt?: string;
  description?: string;
}

/**
 * Singleton registry for agent constructor metadata.
 */
class AgentConstructorRegistryImpl {
  private readonly registry: Map<string, AgentConstructorMetadata>;

  constructor() {
    this.registry = new Map();
  }

  ensureMeta(agentClassName: string): void {
    if (!this.registry.has(agentClassName)) {
      this.registry.set(agentClassName, {});
    }
  }

  lookup(agentClassName: string): AgentConstructorMetadata | undefined {
    return this.registry.get(agentClassName);
  }

  setPrompt(agentClassName: string, prompt: string): void {
    this.ensureMeta(agentClassName);
    const classMeta = this.registry.get(agentClassName)!;
    classMeta.prompt = prompt;
  }

  setDescription(agentClassName: string, description: string): void {
    this.ensureMeta(agentClassName);
    const classMeta = this.registry.get(agentClassName)!;
    classMeta.description = description;
  }
}

export const AgentConstructorRegistry: AgentConstructorRegistryImpl =
  new AgentConstructorRegistryImpl();
