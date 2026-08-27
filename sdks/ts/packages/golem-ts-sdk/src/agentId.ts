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

import { makeAgentId, parseAgentId } from 'golem:agent/host@2.0.0';
import { Uuid } from './uuid';
import { Uuid as RawUuid } from 'golem:core/types@2.0.0';
import { SchemaValue, schemaValueFromWit, schemaValueToWit } from './internal/schema-model';
import { ComponentId } from './ids';

/** Globally unique identity of a Golem agent. */
export interface AgentId {
  readonly componentId: ComponentId;
  readonly agentId: string;
}

/** Explicit parts used to construct an {@link AgentId}. */
export interface AgentIdCreateOptions {
  readonly componentId: ComponentId;
  readonly typeName: string;
  readonly constructorValue: SchemaValue;
  readonly phantomId?: Uuid;
}

/** Parsed semantic parts of an {@link AgentId}. */
export interface AgentIdParts {
  readonly componentId: ComponentId;
  readonly typeName: string;
  readonly constructorValue: SchemaValue;
  readonly phantomId?: Uuid;
}

/** Construct and inspect full agent identities. */
export const AgentId = Object.freeze({
  create(options: AgentIdCreateOptions): AgentId {
    return {
      componentId: options.componentId,
      agentId: createAgentIdString(options.typeName, options.constructorValue, options.phantomId),
    };
  },

  parse(value: AgentId): AgentIdParts {
    const [typeName, typedParameters, rawPhantomId] = parseAgentId(value.agentId);
    return {
      componentId: value.componentId,
      typeName,
      constructorValue: schemaValueFromWit(typedParameters.value),
      phantomId: rawPhantomId ? Uuid.from(rawPhantomId) : undefined,
    };
  },
});

function createAgentIdString(
  agentTypeName: string,
  parameters: SchemaValue,
  phantomId?: RawUuid,
): string {
  const normalized = phantomId ? Uuid.from(phantomId) : undefined;
  return makeAgentId(agentTypeName, schemaValueToWit(parameters), normalized);
}

/**
 * Parsed component-local agent identity string.
 *
 * A ParsedAgentId wraps the string representation of an agent ID and can parse it
 * into its constituent parts: agent type name, constructor parameters, and optional phantom ID.
 *
 * Constructor parameters are carried as the schema-native {@link SchemaValue} (the recursive
 * in-memory value); the embedded type graph the host returns is discarded because the SDK
 * re-derives parameter types from the agent's registered runtime metadata.
 */
export class ParsedAgentId {
  readonly value: string;

  parsedCache: [string, SchemaValue, Uuid | undefined] | undefined = undefined;

  constructor(agentId: string) {
    this.value = agentId;
  }

  /**
   * Constructs a ParsedAgentId from the given agent type name, parameters and an optional phantom ID.
   * @param agentTypeName Agent type name in kebab-case
   * @param parameters Constructor parameter values encoded as a {@link SchemaValue} record
   * @param phantomId Optional phantom ID
   * @internal Use a definition's `agentId(...)` or {@link AgentId.create}.
   */
  static create(
    agentTypeName: string,
    parameters: SchemaValue,
    phantomId?: RawUuid,
  ): ParsedAgentId {
    const normalized = phantomId ? Uuid.from(phantomId) : undefined;
    const value = createAgentIdString(agentTypeName, parameters, normalized);
    const result = new ParsedAgentId(value);
    result.parsedCache = [agentTypeName, parameters, normalized];
    return result;
  }

  /**
   * Returns the parsed agent ID.
   * @returns a tuple of the agent type name, parameters and an optional phantom ID
   */
  parsed(): [string, SchemaValue, Uuid | undefined] {
    if (!this.parsedCache) {
      const [typeName, typedParams, rawPhantomId] = parseAgentId(this.value);
      this.parsedCache = [
        typeName,
        schemaValueFromWit(typedParams.value),
        rawPhantomId ? Uuid.from(rawPhantomId) : undefined,
      ];
    }
    return this.parsedCache;
  }
}
