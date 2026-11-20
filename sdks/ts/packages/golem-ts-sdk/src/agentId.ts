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

import type { AgentId as RawAgentId } from 'golem:core/types@2.0.0';
import { Uuid } from './uuid';

const UUID_PATTERN =
  /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

/**
 * Names an agent within its environment. Golem issues it as an opaque version 8 UUID; it does not
 * encode the agent type's name or the constructor parameters. Its text form is the UUID.
 *
 * Structurally compatible with the WIT `agent-id` record, so it can be passed to host functions
 * directly.
 */
export class AgentId {
  readonly uuid: Uuid;

  constructor(uuid: Uuid) {
    this.uuid = Uuid.from(uuid);
  }

  /** Wraps a WIT `agent-id`. An `AgentId` is returned as it is. */
  static from(value: RawAgentId): AgentId {
    if (value instanceof AgentId) {
      return value;
    }
    return new AgentId(Uuid.from(value.uuid));
  }

  /** Parses the text form, a hyphenated UUID. */
  static parse(value: string): AgentId {
    if (!UUID_PATTERN.test(value)) {
      throw new TypeError(`Invalid agent ID: ${value} is not a UUID`);
    }
    return new AgentId(Uuid.parse(value));
  }

  /** The text form: the hyphenated UUID. */
  toString(): string {
    return this.uuid.toString();
  }

  toJSON(): string {
    return this.toString();
  }

  equals(other: RawAgentId): boolean {
    return this.uuid.highBits === other.uuid.highBits && this.uuid.lowBits === other.uuid.lowBits;
  }
}
