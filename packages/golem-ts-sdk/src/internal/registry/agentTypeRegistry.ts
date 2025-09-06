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
import { AgentClassName } from '../../newTypes/agentClassName';
import * as Option from 'effect/Option';
import { AgentTypeName } from '../../newTypes/agentTypeName';
import { AgentInitiator } from '../agentInitiator';

type AgentClassNameString = string;

const agentTypeRegistry = new Map<AgentClassNameString, AgentType>();

export const AgentTypeRegistry = {
  register(agentClassName: AgentClassName, agentType: AgentType): void {
    agentTypeRegistry.set(agentClassName.value, agentType);
  },

  entries(): IterableIterator<[AgentClassName, AgentType]> {
    return Array.from(agentTypeRegistry.entries())
      .map(
        ([name, agentType]) =>
          [new AgentClassName(name), agentType] as [AgentClassName, AgentType],
      )
      [Symbol.iterator]();
  },

  getRegisteredAgents(): AgentType[] {
    return Array.from(agentTypeRegistry.values());
  },

  lookup(agentClassName: AgentClassName): Option.Option<AgentType> {
    return Option.fromNullable(agentTypeRegistry.get(agentClassName.value));
  },

  exists(agentClassName: AgentClassName): boolean {
    return agentTypeRegistry.has(agentClassName.value);
  },
};
