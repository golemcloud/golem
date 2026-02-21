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

import { ResolvedAgent } from './resolvedAgent';
import { Result } from 'golem:rpc/types@0.2.2';
import { AgentError, DataValue, Principal } from 'golem:agent/common';

/**
 * Any agent creation in SDK across any boundaries goes through `AgentInitiator`
 *
 * AgentInitiator is the canonical interface for instantiating agents.
 * The exported component uses AgentInitiator, and so is remoteClients.
 * AgentInitiator is internal to the SDK, and should not be exposed to end users.
 *
 * An AgentInitiator returns is `ResolvedAgent`, which encapsulates:
 * - The original instance of the user's agent (the instance of the user's class)
 * - And most importantly, an instance of `AgentInternal`, useful for invoking dynamic methods on the agent
 */
export type AgentInitiator = {
  /**
   * Initiates the creation of an agent.
   *
   * @param constructorParams - Constructor arguments for the agent, encoded as `WitValue`s.
   * @returns A `ResolvedAgent` containing the created agent and its internal handler.
   */
  initiate(constructorParams: DataValue, principal: Principal): Result<ResolvedAgent, AgentError>;
};
