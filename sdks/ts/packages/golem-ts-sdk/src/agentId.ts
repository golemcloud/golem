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
import { DynamicAgentClient, type DynamicAgentClientSurface } from './dynamicClient';

/** @internal Protocol implemented by typed contracts and reflected agent types. */
export const bindAgentClient = '__golemBindAgentClient' as const;

/** @internal A value that can bind itself to an existing agent identity. */
export interface AgentClientBinding<Client> {
  [bindAgentClient](agentId: ParsedAgentId): Client;
}

/** Explicit parts used to construct a {@link ParsedAgentId}. */
export interface ParsedAgentIdCreateOptions {
  readonly typeName: string;
  readonly constructorValue: SchemaValue;
  readonly phantomId?: Uuid;
}

/** Semantic parts of a {@link ParsedAgentId}. */
export interface ParsedAgentIdParts {
  readonly typeName: string;
  readonly constructorValue: SchemaValue;
  readonly phantomId?: Uuid;
}

function createAgentIdString(
  agentTypeName: string,
  parameters: SchemaValue,
  phantomId?: RawUuid,
): string {
  const normalized = phantomId ? Uuid.from(phantomId) : undefined;
  return makeAgentId(agentTypeName, schemaValueToWit(parameters), normalized);
}

/**
 * Parsed environment-scoped agent identity string.
 *
 * A ParsedAgentId wraps the environment-scoped string representation of an agent ID and can parse it
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
   * Constructs a ParsedAgentId from an agent type name, constructor value, and optional phantom ID.
   * Prefer a definition's `agentId(...)` when a typed definition is available.
   */
  static create(options: ParsedAgentIdCreateOptions): ParsedAgentId {
    const normalized = options.phantomId ? Uuid.from(options.phantomId) : undefined;
    const value = createAgentIdString(options.typeName, options.constructorValue, normalized);
    const result = new ParsedAgentId(value);
    result.parsedCache = [options.typeName, options.constructorValue, normalized];
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

  /** Return the semantic parts of this environment-scoped identity. */
  parts(): ParsedAgentIdParts {
    const [typeName, constructorValue, phantomId] = this.parsed();
    return { typeName, constructorValue, phantomId };
  }

  /** Bind caller-supplied codecs or a reflected agent type to this identity. */
  client<Client>(binding: AgentClientBinding<Client>): Client {
    const bind = binding[bindAgentClient];
    if (typeof bind !== 'function') {
      throw new TypeError('Expected an agent client contract or reflected agent type');
    }
    return bind.call(binding, this);
  }

  /** Invoke this identity with schema values and no discovery or typed contract. */
  dynamicClient(): DynamicAgentClientSurface {
    return new DynamicAgentClient(this);
  }
}
