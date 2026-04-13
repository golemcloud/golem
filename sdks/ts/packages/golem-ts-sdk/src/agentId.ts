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

import { DataValue, makeAgentId, parseAgentId } from 'golem:agent/host@1.5.0';
import { Uuid } from './uuid';
import { Uuid as RawUuid } from 'golem:core/types@1.5.0';

/**
 * Globally unique ID of an `agent`.
 *
 * A ParsedAgentId wraps the string representation of an agent ID and can parse it
 * into its constituent parts: agent type name, constructor parameters, and optional phantom ID.
 */
export class ParsedAgentId {
  readonly value: string;

  parsedCache: [string, DataValue, Uuid | undefined] | undefined = undefined;

  constructor(agentId: string) {
    this.value = agentId;
  }

  /**
   * Constructs a ParsedAgentId from the given agent type name, parameters and an optional phantom ID.
   * @param agentTypeName Agent type name in kebab-case
   * @param parameters Constructor parameter values encoded as DataValue
   * @param phantomId Optional phantom ID
   */
  static make(agentTypeName: string, parameters: DataValue, phantomId?: RawUuid): ParsedAgentId {
    const normalized = phantomId ? Uuid.from(phantomId) : undefined;
    const value = makeAgentId(agentTypeName, parameters, normalized);
    const result = new ParsedAgentId(value);
    result.parsedCache = [agentTypeName, parameters, normalized];
    return result;
  }

  /**
   * Returns the parsed agent ID.
   * @returns a tuple of the agent type name, parameters and an optional phantom ID
   */
  parsed(): [string, DataValue, Uuid | undefined] {
    if (!this.parsedCache) {
      const [typeName, params, rawPhantomId] = parseAgentId(this.value);
      this.parsedCache = [typeName, params, rawPhantomId ? Uuid.from(rawPhantomId) : undefined];
    }
    return this.parsedCache;
  }
}
