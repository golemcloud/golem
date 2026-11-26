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
 * encode the agent type's name or the constructor parameters.
 *
 * An agent ID is its lower-case hyphenated UUID text, branded so that other strings are not
 * accepted where an agent ID is expected. Compare IDs with `===`, use them as `Map` keys, and
 * store them as text; `AgentId.parse` reads the text back.
 */
// The brand is a string key, not a unique symbol, so that the type is the same in every bundled
// declaration entrypoint.
export type AgentId = string & { readonly __brand: 'golem.AgentId' };

export const AgentId = {
  /**
   * Parses the text form, a hyphenated UUID. Upper-case hex is accepted and the result is lower
   * case. Throws a `TypeError` for other text. The parse checks only the text form: whether an
   * agent has the ID is known only to Golem, for example through a client's `getById`.
   */
  parse(value: string): AgentId {
    if (!UUID_PATTERN.test(value)) {
      throw new TypeError(`Invalid agent ID: ${value} is not a UUID`);
    }
    return value.toLowerCase() as AgentId;
  },

  /** The agent ID of a WIT `agent-id` record. */
  fromWit(value: RawAgentId): AgentId {
    return AgentId.parse(Uuid.from(value.uuid).toString());
  },

  /** The WIT `agent-id` record of an agent ID, for a call of a host function. */
  toWit(value: AgentId): RawAgentId {
    return { uuid: Uuid.parse(value) };
  },
};
