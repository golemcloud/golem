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

import { TypeInfoInternal } from '../typeInfoInternal';
import { HttpEndpointDetails } from 'golem:agent/common@1.5.0';

export interface AgentMethodMetadata {
  prompt?: string;
  description?: string;
  returnType?: TypeInfoInternal;
  httpEndpoint?: HttpEndpointDetails[];
}

/**
 * Singleton registry for agent method metadata.
 */
class AgentMethodRegistryImpl {
  private readonly registry: Map<string, Map<string, AgentMethodMetadata>>;

  constructor() {
    this.registry = new Map();
  }

  ensureMeta(agentClassName: string, method: string): void {
    if (!this.registry.has(agentClassName)) {
      this.registry.set(agentClassName, new Map());
    }
    const classMeta = this.registry.get(agentClassName)!;
    if (!classMeta.has(method)) {
      classMeta.set(method, {});
    }
  }

  get(agentClassName: string): Map<string, AgentMethodMetadata> | undefined {
    return this.registry.get(agentClassName);
  }

  getReturnType(agentClassName: string, agentMethodName: string): TypeInfoInternal | undefined {
    const classMeta = this.registry.get(agentClassName);
    return classMeta?.get(agentMethodName)?.returnType;
  }

  setPrompt(agentClassName: string, method: string, prompt: string): void {
    this.ensureMeta(agentClassName, method);
    const classMeta = this.registry.get(agentClassName)!;
    classMeta.get(method)!.prompt = prompt;
  }

  setDescription(agentClassName: string, method: string, description: string): void {
    this.ensureMeta(agentClassName, method);
    const classMeta = this.registry.get(agentClassName)!;
    classMeta.get(method)!.description = description;
  }

  setReturnType(agentClassName: string, method: string, returnType: TypeInfoInternal): void {
    this.ensureMeta(agentClassName, method);
    const classMeta = this.registry.get(agentClassName)!;
    classMeta.get(method)!.returnType = returnType;
  }

  setHttpEndpoint(agentClassName: string, method: string, endpoint: HttpEndpointDetails): void {
    this.ensureMeta(agentClassName, method);
    const classMeta = this.registry.get(agentClassName)!;
    classMeta.get(method)!.httpEndpoint = classMeta.get(method)!.httpEndpoint || [];
    classMeta.get(method)!.httpEndpoint!.push(endpoint);
  }

  debugDump(): void {
    console.log(JSON.stringify(this.registry));
  }
}

export const AgentMethodRegistry: AgentMethodRegistryImpl = new AgentMethodRegistryImpl();
