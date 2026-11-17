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

import type { SchemaValueTree } from 'golem:core/types@2.0.0';
import { schemaValueToWit, type SchemaValue } from '../src/internal/schema-model';
import { encodeConstructorParameters } from '../src/internal/snapshotState';

/** The agent ID that the mocked `GOLEM_AGENT_ID` gives the agent under test. */
export const SELF_AGENT_ID = '00000007-0000-8000-8000-000000000001';

/** Makes the mocked environment describe an agent of `typeName`, as Golem does. */
export function selectSelfAgent(typeName: string): void {
  (globalThis as { currentAgentType?: string }).currentAgentType = typeName;
  (globalThis as { currentAgentId?: string }).currentAgentId = SELF_AGENT_ID;
}

/** The `constructorParameters` field of a snapshot envelope for these parameters. */
export function constructorParametersJson(value: SchemaValue | SchemaValueTree): unknown {
  return encodeConstructorParameters('valueNodes' in value ? value : schemaValueToWit(value));
}
