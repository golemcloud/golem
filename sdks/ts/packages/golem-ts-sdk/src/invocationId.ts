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

import type { Uuid as RawUuid } from 'golem:core/types@2.0.0';
import type {
  CancellationToken,
  InvocationMetadata as HostInvocationMetadata,
} from 'golem:agent/host@2.0.0';
import { Uuid } from './uuid';

const UUID_PATTERN =
  /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;
const UUID_TEXT_LENGTH = 36;

/**
 * Identifies one invocation of an agent: the agent ID text form together with the invocation's
 * UUID. The text form is `<agent-id>.<uuid>`.
 */
export class InvocationId {
  readonly agentId: string;
  readonly uuid: Uuid;

  constructor(agentId: string, uuid: RawUuid) {
    if (agentId.length === 0) {
      throw new Error('Invalid invocation ID: the agent ID is empty');
    }
    this.agentId = agentId;
    this.uuid = Uuid.from(uuid);
  }

  /** Formats the invocation ID as `<agent-id>.<uuid>`. */
  toString(): string {
    return `${this.agentId}.${this.uuid.toString()}`;
  }

  /**
   * Parses the `<agent-id>.<uuid>` text form. The UUID and its separator are the last 37
   * characters, so the agent ID itself can contain `.`.
   */
  static parse(value: string): InvocationId {
    const separatorIndex = value.length - UUID_TEXT_LENGTH - 1;
    if (separatorIndex < 0 || value[separatorIndex] !== '.') {
      throw new Error(`Invalid invocation ID: missing '.' before the UUID in ${value}`);
    }
    const uuidText = value.slice(separatorIndex + 1);
    if (!UUID_PATTERN.test(uuidText)) {
      throw new Error(`Invalid invocation ID: invalid UUID ${uuidText}`);
    }
    const hex = uuidText.replace(/-/g, '');
    const uuid = new Uuid(BigInt(`0x${hex.slice(0, 16)}`), BigInt(`0x${hex.slice(16)}`));
    return new InvocationId(value.slice(0, separatorIndex), uuid);
  }
}

/**
 * Final identity allocated for one remote invocation. For an ephemeral target, `agentId`
 * contains the generated one-shot phantom ID.
 */
export interface InvocationMetadata {
  readonly agentId: string;
  readonly invocationId: InvocationId;
}

/** Receipt returned when an invocation has been scheduled. */
export interface ScheduledInvocationReceipt {
  readonly metadata: InvocationMetadata;
}

/** Receipt for a scheduled invocation that can still be cancelled. */
export interface CancelableScheduledInvocationReceipt {
  readonly metadata: InvocationMetadata;
  readonly cancellationToken: CancellationToken;
}

/** @internal Converts the host invocation metadata into the frozen SDK form. */
export function invocationMetadataFromHost(metadata: HostInvocationMetadata): InvocationMetadata {
  return Object.freeze({
    agentId: metadata.agentId,
    invocationId: new InvocationId(metadata.agentId, metadata.invocationId.id),
  });
}
