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

import { AgentClassName } from '../../agentClassName';
import { TypeInfoInternal } from '../typeInfoInternal';
import * as util from 'node:util';

export interface AgentMethodParamMetadata {
  typeInfo?: TypeInfoInternal;
}

/**
 * Singleton registry for agent method parameter metadata.
 */
class AgentMethodParamRegistryImpl {
  private readonly registry: Map<string, Map<string, Map<string, AgentMethodParamMetadata>>>;

  constructor() {
    this.registry = new Map();
  }

  ensureMeta(agentClassName: string, method: string, paramName: string): void {
    if (!this.registry.has(agentClassName)) {
      this.registry.set(agentClassName, new Map());
    }
    const classMeta = this.registry.get(agentClassName)!;
    if (!classMeta.has(method)) {
      classMeta.set(method, new Map());
    }

    const methodMeta = classMeta.get(method)!;

    if (!methodMeta.has(paramName)) {
      methodMeta.set(paramName, {});
    }
  }

  get(agentClassName: string): Map<string, Map<string, AgentMethodParamMetadata>> | undefined {
    if (!this.registry.has(agentClassName)) {
      this.registry.set(agentClassName, new Map());
    }
    return this.registry.get(agentClassName);
  }

  getParametersAndType(
    agentClassName: string,
    agentMethodName: string,
  ): Map<string, TypeInfoInternal> {
    const result = new Map<string, TypeInfoInternal>();

    const classMeta = this.registry.get(agentClassName);

    const methodMeta = classMeta?.get(agentMethodName);

    methodMeta?.forEach((paramMeta, paramName) => {
      if (paramMeta.typeInfo) {
        result.set(paramName, paramMeta.typeInfo!);
      }
    });

    return result;
  }

  getParamType(
    agentClassName: string,
    agentMethodName: string,
    paramName: string,
  ): TypeInfoInternal | undefined {
    const classMeta = this.registry.get(agentClassName);
    return classMeta?.get(agentMethodName)?.get(paramName)?.typeInfo;
  }

  setType(
    agentClassName: string,
    agentMethodName: string,
    paramName: string,
    typeInfo: TypeInfoInternal,
  ): void {
    this.ensureMeta(agentClassName, agentMethodName, paramName);
    const classMeta = this.registry.get(agentClassName)!;
    const methodMeta = classMeta.get(agentMethodName)!;
    methodMeta.get(paramName)!.typeInfo = typeInfo;
  }

  debugDump(): void {
    console.log(JSON.stringify(this.registry));
  }
}

export const AgentMethodParamRegistry: AgentMethodParamRegistryImpl =
  new AgentMethodParamRegistryImpl();
