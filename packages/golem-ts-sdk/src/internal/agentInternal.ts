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

import { Result } from 'golem:rpc/types@0.2.2';
import { AgentError, AgentType, DataValue } from 'golem:agent/common';
import { AgentId } from '../agentId';

/**
 * An AgentInternal is an internal interface that represents the basic usage of an agent
 * It is constructed only after instantiating of an agent through the AgentInitiator.
 */
export interface AgentInternal {
  getId(): AgentId;
  invoke(
    method: string,
    args: DataValue,
  ): Promise<Result<DataValue, AgentError>>;
  getAgentType(): AgentType;
}
