// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

import { Result } from 'golem:agent/host@2.0.0';
import { AgentError, AgentType, Principal } from 'golem:agent/common@2.0.0';
import { SchemaValueTree } from 'golem:core/types@2.0.0';
import { ParsedAgentId } from '../agentId';

/**
 * The minimal resolved-agent contract the guest runtime (`src/index.ts`) drives:
 * invoke a method, describe the agent type, and save/load snapshots. Produced by
 * an {@link AgentInitiator} and implemented by the runtime's resolved agent
 * (`src/agent/runtime.ts` `FluentResolvedAgent`).
 */
export interface ResolvedAgent {
  getId(): ParsedAgentId;
  getAgentType(): AgentType;
  invoke(
    methodName: string,
    methodArgs: SchemaValueTree,
    principal: Principal,
  ): Promise<Result<SchemaValueTree | undefined, AgentError>>;
  saveSnapshot(): Promise<{ data: Uint8Array; mimeType: string }>;
  loadSnapshot(bytes: Uint8Array, mimeType?: string): Promise<void>;
}
