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

interface AgentConstructorParamMetadata {
  typeInfo?: TypeInfoInternal;
}

/**
 * Singleton registry for agent constructor parameter metadata.
 */
class AgentConstructorParamRegistryImpl {
  private readonly registry: Map<string, Map<string, AgentConstructorParamMetadata>>;

  constructor() {
    this.registry = new Map();
  }

  ensureMeta(agentClassName: string, paramName: string): void {
    if (!this.registry.has(agentClassName)) {
      this.registry.set(agentClassName, new Map());
    }
    const classMeta = this.registry.get(agentClassName)!;
    if (!classMeta.has(paramName)) {
      classMeta.set(paramName, {});
    }
  }

  get(agentClassName: string): Map<string, AgentConstructorParamMetadata> | undefined {
    return this.registry.get(agentClassName);
  }

  getParametersForPrincipal(agentClassName: string): Set<string> {
    const classMeta = this.registry.get(agentClassName);

    const principalParams: Set<string> = new Set();

    classMeta?.forEach((param, paramName) => {
      if (param.typeInfo?.tag === 'principal') {
        principalParams.add(paramName);
      }
    });

    return principalParams;
  }

  getParamType(agentClassName: string, paramName: string): TypeInfoInternal | undefined {
    const classMeta = this.registry.get(agentClassName);
    return classMeta?.get(paramName)?.typeInfo;
  }

  setType(agentClassName: string, paramName: string, typeInfo: TypeInfoInternal): void {
    this.ensureMeta(agentClassName, paramName);
    const classMeta = this.registry.get(agentClassName)!;
    classMeta.get(paramName)!.typeInfo = typeInfo;
  }
}

export const AgentConstructorParamRegistry: AgentConstructorParamRegistryImpl =
  new AgentConstructorParamRegistryImpl();
