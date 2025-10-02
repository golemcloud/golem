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

import { AgentClassName } from '../../newTypes/agentClassName';
import { TypeInfoInternal } from './typeInfoInternal';

type AgentClassNameString = string;
type ParamName = string;

const agentConstructorParamRegistry = new Map<
  AgentClassNameString,
  Map<
    ParamName,
    {
      typeInfo?: TypeInfoInternal;
    }
  >
>();

export const AgentConstructorParamRegistry = {
  ensureMeta(agentClassName: AgentClassName, paramName: string) {
    if (!agentConstructorParamRegistry.has(agentClassName.value)) {
      agentConstructorParamRegistry.set(agentClassName.value, new Map());
    }
    const classMeta = agentConstructorParamRegistry.get(agentClassName.value)!;
    if (!classMeta.has(paramName)) {
      classMeta.set(paramName, {});
    }
  },

  get(agentClassName: AgentClassName) {
    return agentConstructorParamRegistry.get(agentClassName.value);
  },

  getParamType(
    agentClassName: AgentClassName,
    paramName: string,
  ): TypeInfoInternal | undefined {
    const classMeta = agentConstructorParamRegistry.get(agentClassName.value);
    return classMeta?.get(paramName)?.typeInfo;
  },

  setType(
    agentClassName: AgentClassName,
    paramName: string,
    typeInfo: TypeInfoInternal,
  ) {
    AgentConstructorParamRegistry.ensureMeta(agentClassName, paramName);
    const classMeta = agentConstructorParamRegistry.get(agentClassName.value)!;
    classMeta.get(paramName)!.typeInfo = typeInfo;
  },

};
