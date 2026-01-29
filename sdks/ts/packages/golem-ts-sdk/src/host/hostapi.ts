// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import { PromiseId, getPromise, Uuid } from 'golem:api/host@1.3.0';
import { parseUuid } from 'golem:rpc/types@0.2.2';
import { AgentId } from '../agentId';
import * as wasiEnv from 'wasi:cli/environment@0.2.3';

// reexport golem host api
export * from 'golem:api/host@1.3.0';

export async function awaitPromise(promiseId: PromiseId): Promise<Uint8Array> {
  const promise = getPromise(promiseId);
  await promise.subscribe().promise();
  return promise.get()!;
}

/**
 *  Generates a new random Golem Uuid
 */
export function randomUuid(): Uuid {
  const uuidString = crypto.randomUUID();
  return parseUuid(uuidString);
}

/**
 * Returns the raw string agent ID of the current agent.
 */
export function getRawSelfAgentId(): AgentId {
  const env = wasiEnv.getEnvironment();
  const agentId: [string, string] | undefined = env.find(([key, _]) => key === 'GOLEM_AGENT_ID');
  if (!agentId) {
    throw new Error('GOLEM_AGENT_ID is not set');
  }
  return new AgentId(agentId[1]);
}
