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

import { AgentType } from 'golem:agent/common@2.0.0';
import { AgentClassName } from '../../agentClassName';

/**
 * Singleton registry for agent types.
 */
class AgentTypeRegistryImpl {
  private readonly registry: Map<string, AgentType>;
  private readonly classNameCache: Map<string, AgentClassName>;
  private readonly registrationErrors: Map<string, string[]>;
  private readonly registrationsInProgress: Set<string>;
  private cachedAgents: AgentType[] | null = null;

  constructor() {
    this.registry = new Map();
    this.classNameCache = new Map();
    this.registrationErrors = new Map();
    this.registrationsInProgress = new Set();
  }

  register(agentClassName: AgentClassName, agentType: AgentType): void {
    this.beginRegistration(agentClassName);
    this.completeRegistration(agentClassName, agentType);
  }

  beginRegistration(agentClassName: AgentClassName): void {
    const nameValue = agentClassName.value;
    if (this.registry.has(nameValue) || this.registrationsInProgress.has(nameValue)) {
      throw new Error(`Agent "${nameValue}" is already registered`);
    }
    this.registrationsInProgress.add(nameValue);
  }

  completeRegistration(agentClassName: AgentClassName, agentType: AgentType): void {
    const nameValue = agentClassName.value;
    if (!this.registrationsInProgress.delete(nameValue) || this.registry.has(nameValue)) {
      throw new Error(`Agent "${nameValue}" does not have a pending registration`);
    }
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
    return (
      this.registry.has(agentClassName.value) ||
      this.registrationsInProgress.has(agentClassName.value)
    );
  }

  recordRegistrationError(agentTypeName: string, message: string): void {
    const messages = this.registrationErrors.get(agentTypeName) ?? [];
    if (!messages.includes(message)) messages.push(message);
    this.registrationErrors.set(agentTypeName, messages);
  }

  getRegistrationError(agentTypeName: string): readonly string[] | undefined {
    return this.registrationErrors.get(agentTypeName);
  }

  getRegistrationErrors(): ReadonlyArray<{
    agentTypeName: string;
    messages: readonly string[];
  }> {
    return Array.from(this.registrationErrors, ([agentTypeName, messages]) => ({
      agentTypeName,
      messages,
    }));
  }
}

export const AgentTypeRegistry: AgentTypeRegistryImpl = new AgentTypeRegistryImpl();
