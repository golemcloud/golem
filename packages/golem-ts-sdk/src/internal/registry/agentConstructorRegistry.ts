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

type AgentClassNameString = string;

const agentConstructorRegistry = new Map<
  AgentClassNameString,
  {
    multimodal?: boolean;
  }
>();

export const AgentConstructorRegistry = {
  ensureMeta(agentClassName: AgentClassName) {
    if (!agentConstructorRegistry.has(agentClassName.value)) {
      agentConstructorRegistry.set(agentClassName.value, {});
    }
  },

  lookup(agentClassName: AgentClassName) {
    return agentConstructorRegistry.get(agentClassName.value);
  },

  setAsMultiModal(agentClassName: AgentClassName) {
    AgentConstructorRegistry.ensureMeta(agentClassName);
    const classMeta = agentConstructorRegistry.get(agentClassName.value)!;
    classMeta.multimodal = true;
  },
};
