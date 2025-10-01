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

import * as Option from '../../newTypes/option';
import { AgentInitiator } from '../agentInitiator';

// Although only 1 agent instance can exist max in a container,
// the container will end up keeping track of initiators of all agent classes
// in the user code for obvious reasons
const agentInitiators = new Map<string, AgentInitiator>();

export const AgentInitiatorRegistry = {
  register(agentTypeName: string, agentInitiator: AgentInitiator): void {
    agentInitiators.set(agentTypeName, agentInitiator);
  },

  lookup(agentTypeName: string): Option.Option<AgentInitiator> {
    return Option.fromNullable(agentInitiators.get(agentTypeName));
  },

  entries(): IterableIterator<[string, AgentInitiator]> {
    return agentInitiators.entries();
  },

  agentTypeNames(): Array<string> {
    return Array.from(agentInitiators.keys());
  },

  exists(agentTypeName: string): boolean {
    return agentInitiators.has(agentTypeName);
  },
};
