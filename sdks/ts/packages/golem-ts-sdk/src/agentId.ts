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

import { DataValue, makeAgentId, parseAgentId } from 'golem:agent/host@1.5.0';
import { Uuid } from 'golem:api/host@1.5.0';

/**
 * Globally unique ID of an `agent`.
 *
 * An AgentId can also be considered as the container-id in which the agent runs.
 */
export class AgentId {
  readonly value: string;

  parsedCache: [string, DataValue, Uuid | undefined] | undefined = undefined;

  constructor(agentId: string) {
    this.value = agentId;
  }

  /**
   * Constructs an AgentId from the given agent type name, parameters and an optional phantom ID.
   * @param agentTypeName Agent type name in kebab-case
   * @param parameters Constructor parameter values encoded as DataValue
   * @param phantomId Optional phantom ID
   */
  static make(agentTypeName: string, parameters: DataValue, phantomId?: Uuid): AgentId {
    const value = makeAgentId(agentTypeName, parameters, phantomId);
    const result = new AgentId(value);
    result.parsedCache = [agentTypeName, parameters, phantomId];
    return result;
  }

  /**
   * Returns the parsed agent ID.
   * @returns a tuple of the agent type name, parameters and an optional phantom ID
   */
  parsed(): [string, DataValue, Uuid | undefined] {
    if (!this.parsedCache) {
      this.parsedCache = parseAgentId(this.value);
    }
    return this.parsedCache;
  }
}
