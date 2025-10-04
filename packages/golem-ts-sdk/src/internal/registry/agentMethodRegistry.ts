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
type AgentMethodNameString = string;

const agentMethodRegistry = new Map<
  AgentClassNameString,
  Map<
    AgentMethodNameString,
    {
      prompt?: string;
      description?: string;
      multimodal?: boolean;
      returnType?: TypeInfoInternal;
    }
  >
>();

export const AgentMethodRegistry = {
  ensureMeta(agentClassName: AgentClassName, method: string) {
    if (!agentMethodRegistry.has(agentClassName.value)) {
      agentMethodRegistry.set(agentClassName.value, new Map());
    }
    const classMeta = agentMethodRegistry.get(agentClassName.value)!;
    if (!classMeta.has(method)) {
      classMeta.set(method, {});
    }
  },

  get(agentClassName: AgentClassName) {
    return agentMethodRegistry.get(agentClassName.value);
  },

  getReturnType(
    agentClassName: AgentClassName,
    agentMethodName: string,
  ): TypeInfoInternal | undefined {
    const classMeta = agentMethodRegistry.get(agentClassName.value);
    return classMeta?.get(agentMethodName)?.returnType;
  },

  setPrompt(agentClassName: AgentClassName, method: string, prompt: string) {
    AgentMethodRegistry.ensureMeta(agentClassName, method);
    const classMeta = agentMethodRegistry.get(agentClassName.value)!;
    classMeta.get(method)!.prompt = prompt;
  },

  setDescription(
    agentClassName: AgentClassName,
    method: string,
    description: string,
  ) {
    AgentMethodRegistry.ensureMeta(agentClassName, method);
    const classMeta = agentMethodRegistry.get(agentClassName.value)!;
    classMeta.get(method)!.description = description;
  },

  setReturnType(
    agentClassName: AgentClassName,
    method: string,
    returnType: TypeInfoInternal,
  ) {
    AgentMethodRegistry.ensureMeta(agentClassName, method);
    const classMeta = agentMethodRegistry.get(agentClassName.value)!;
    classMeta.get(method)!.returnType = returnType;
  },

  setAsMultimodal(agentClassName: AgentClassName, method: string) {
    AgentMethodRegistry.ensureMeta(agentClassName, method);
    const classMeta = agentMethodRegistry.get(agentClassName.value)!;
    classMeta.get(method)!.multimodal = true;
  },
};
