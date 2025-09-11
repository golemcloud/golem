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

import { AgentTypeName } from './newTypes/agentTypeName';

/**
 * Globally unique ID of an `agent`.
 *
 * An AgentId can also be considered as the container-id in which the agent runs.
 * This is because an AgentId must be distinguished in terms of string representation
 * of an agent-type and its constructor parameters.
 *
 * TODO: Once the host exports a canonical way of constructing the worker-name (or container-name)
 * we should use it form the agent-id
 *
 */
export class AgentId {
  readonly value: string;

  constructor(agentId: string) {
    this.value = agentId;
  }

  // !!!! This is only a placeholder implementation (simplest)
  // as we are yet to decide on how to handle values of complex types
  static fromAgentTypeAndParams(
    agentType: AgentTypeName,
    params: any[],
  ): AgentId {
    // Only placeholder implementation
    const paramsConcatenated: string[] = params.map((param) =>
      param.toString(),
    );

    const param = paramsConcatenated.join(',');
    return new AgentId(
      paramsConcatenated.length === 0
        ? agentType.toString()
        : `${agentType.toString()}-{${param}}`,
    );
  }
}
