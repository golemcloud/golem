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

import type { AgentId as RawAgentId, Uuid as RawUuid } from 'golem:core/types@2.0.0';
import type { InvocationMetadata as HostInvocationMetadata } from 'golem:agent/host@2.0.0';
import { AgentId } from './agentId';
import { Uuid } from './uuid';

const UUID_PATTERN =
  /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;
const UUID_TEXT_LENGTH = 36;

function parseUuidText(text: string, what: string): Uuid {
  if (!UUID_PATTERN.test(text)) {
    throw new Error(`Invalid invocation ID: invalid ${what} ${text}`);
  }
  const hex = text.replace(/-/g, '');
  return new Uuid(BigInt(`0x${hex.slice(0, 16)}`), BigInt(`0x${hex.slice(16)}`));
}

/**
 * Identifies one invocation of an agent: the agent's ID together with the invocation's UUID.
 * The text form is `<agent-id>.<uuid>`, where both parts are UUIDs.
 */
export class InvocationId {
  readonly agentId: AgentId;
  readonly uuid: Uuid;

  constructor(agentId: AgentId, uuid: RawUuid) {
    this.agentId = AgentId.parse(agentId);
    this.uuid = Uuid.from(uuid);
  }

  /** Formats the invocation ID as `<agent-id>.<uuid>`. */
  toString(): string {
    return `${this.agentId}.${this.uuid.toString()}`;
  }

  /** Parses the `<agent-id>.<uuid>` text form. */
  static parse(value: string): InvocationId {
    if (value.length !== 2 * UUID_TEXT_LENGTH + 1 || value[UUID_TEXT_LENGTH] !== '.') {
      throw new Error(`Invalid invocation ID: expected <agent-id>.<uuid>, got ${value}`);
    }
    const agentUuid = parseUuidText(value.slice(0, UUID_TEXT_LENGTH), 'agent ID');
    const uuid = parseUuidText(value.slice(UUID_TEXT_LENGTH + 1), 'UUID');
    return new InvocationId(AgentId.fromWit({ uuid: agentUuid }), uuid);
  }
}

/**
 * Final identity allocated for one awaited remote invocation. For an ephemeral target, `agentId`
 * is the ID of the one-shot instance that ran it.
 */
export interface InvocationMetadata {
  readonly agentId: AgentId;
  readonly invocationId: InvocationId;
}

/** @internal Converts the host invocation metadata into the frozen SDK form. */
export function invocationMetadataFromHost(metadata: HostInvocationMetadata): InvocationMetadata {
  const agentId = AgentId.fromWit(metadata.agentId);
  return Object.freeze({
    agentId,
    invocationId: new InvocationId(agentId, metadata.invocationId.id),
  });
}
