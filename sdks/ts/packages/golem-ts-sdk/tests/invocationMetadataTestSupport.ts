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

import type {
  InvocationMetadata as HostInvocationMetadata,
  InvocationStatus,
} from 'golem:agent/host@2.0.0';
import type { SchemaValueTree } from 'golem:core/types@2.0.0';
import { vi } from 'vitest';
import { AgentId } from '../src/agentId';
import { InvocationId } from '../src/invocationId';
import { Uuid } from '../src/uuid';

/** The invocation UUID used by host invocation metadata fakes. */
export const testInvocationUuid = new Uuid(0x0123456789abcdefn, 0xfedcba9876543210n);

/** The agent ID that host invocation metadata fakes name by default. */
export const TEST_INVOKED_AGENT_ID = '00000007-0000-8000-8000-0000000000aa';

/** Host invocation metadata as `golem:agent/host` returns it. */
export function hostInvocationMetadata(
  agentId: string = TEST_INVOKED_AGENT_ID,
): HostInvocationMetadata {
  const raw = AgentId.toWit(AgentId.parse(agentId));
  return {
    agentId: { uuid: { highBits: raw.uuid.highBits, lowBits: raw.uuid.lowBits } },
    invocationId: {
      owner: {
        tag: 'agent',
        val: { uuid: { highBits: raw.uuid.highBits, lowBits: raw.uuid.lowBits } },
      },
      id: { highBits: testInvocationUuid.highBits, lowBits: testInvocationUuid.lowBits },
    },
  };
}

/** The SDK invocation metadata that {@link hostInvocationMetadata} converts to. */
export function sdkInvocationMetadata(agentId: string = TEST_INVOKED_AGENT_ID) {
  const id = AgentId.parse(agentId);
  return { agentId: id, invocationId: new InvocationId(id, testInvocationUuid) };
}

/** A completed `invoke-and-await` as `golem:agent/host` returns it. */
export function hostInvocationResult(result?: SchemaValueTree, agentId?: string) {
  return { metadata: hostInvocationMetadata(agentId), result };
}

/** The host invocation ID that {@link hostInvocationMetadata} names. */
export function hostInvocationId(agentId: string = TEST_INVOKED_AGENT_ID) {
  return hostInvocationMetadata(agentId).invocationId;
}

/** A fake host `invocation` resource whose async methods are mocks. */
export function fakeHostInvocation(agentId: string = TEST_INVOKED_AGENT_ID) {
  return {
    id: vi.fn(() => hostInvocationId(agentId)),
    status: vi.fn(async (): Promise<InvocationStatus> => 'pending'),
    await_: vi.fn(async (): Promise<SchemaValueTree | undefined> => undefined),
    cancel: vi.fn(async () => true),
  };
}
