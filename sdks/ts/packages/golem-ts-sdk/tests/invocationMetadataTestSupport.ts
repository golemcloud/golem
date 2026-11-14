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

import type { InvocationMetadata as HostInvocationMetadata } from 'golem:agent/host@2.0.0';
import { InvocationId } from '../src/invocationId';
import { Uuid } from '../src/uuid';

/** The invocation UUID used by host invocation metadata fakes. */
export const testInvocationUuid = new Uuid(0x0123456789abcdefn, 0xfedcba9876543210n);

/** Host invocation metadata as `golem:agent/host` returns it. */
export function hostInvocationMetadata(agentId: string): HostInvocationMetadata {
  return {
    agentId,
    invocationId: {
      owner: {
        tag: 'agent',
        val: { componentId: { uuid: { highBits: 0n, lowBits: 1n } }, agentId },
      },
      id: { highBits: testInvocationUuid.highBits, lowBits: testInvocationUuid.lowBits },
    },
  };
}

/** The SDK invocation metadata that {@link hostInvocationMetadata} converts to. */
export function sdkInvocationMetadata(agentId: string) {
  return { agentId, invocationId: new InvocationId(agentId, testInvocationUuid) };
}
