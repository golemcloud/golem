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

import * as Option from 'effect/Option';
import { AgentInitiator } from '../agentInitiator';
import { AgentTypeName } from '../../newTypes/agentTypeName';

// Although only 1 agent instance can exist max in a container,
// the container will end up keeping track of initiators of all agent classes
// in the user code for obvious reasons
const agentInitiators = new Map<string, AgentInitiator>();

export const AgentInitiatorRegistry = {
  register(agentName: AgentTypeName, agentInitiator: AgentInitiator): void {
    agentInitiators.set(agentName.value, agentInitiator);
  },

  lookup(agentName: AgentTypeName): Option.Option<AgentInitiator> {
    return Option.fromNullable(agentInitiators.get(agentName.value));
  },

  entries(): IterableIterator<[AgentTypeName, AgentInitiator]> {
    return Array.from(agentInitiators.entries())
      .map(
        ([name, initiator]) =>
          [new AgentTypeName(name), initiator] as [
            AgentTypeName,
            AgentInitiator,
          ],
      )
      [Symbol.iterator]();
  },
};
