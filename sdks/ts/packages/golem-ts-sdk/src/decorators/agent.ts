// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

import {
  AgentType,
  DataValue,
  AgentConstructor,
  AgentMode,
  Principal,
  Snapshotting,
} from 'golem:agent/common';
import { ResolvedAgent } from '../internal/resolvedAgent';
import { TypeMetadata } from '@golemcloud/golem-ts-types-core';
import {
  getNewPhantomRemoteClient,
  getPhantomRemoteClient,
  getRemoteClient,
} from '../internal/clientGeneration';
import { BaseAgent } from '../baseAgent';
import { AgentTypeRegistry } from '../internal/registry/agentTypeRegistry';
import * as Either from '../newTypes/either';
import { AgentClassName } from '../agentClassName';
import { AgentInitiatorRegistry } from '../internal/registry/agentInitiatorRegistry';
import { createCustomError } from '../internal/agentError';
import { AgentConstructorParamRegistry } from '../internal/registry/agentConstructorParamRegistry';
import { AgentConstructorRegistry } from '../internal/registry/agentConstructorRegistry';
import { deserializeDataValue, ParameterDetail } from '../internal/mapping/values/dataValue';
import { getRawSelfAgentId } from '../host/hostapi';
import { getHttpMountDetails } from '../internal/http/mount';
import { validateHttpMount } from '../internal/http/validation';
import { getAgentConstructorSchema } from '../internal/schema/constructor';
import { getAgentMethodSchema } from '../internal/schema/method';
import ms from 'ms';

export type SnapshottingOption = 'disabled' | 'enabled' | { periodic: string } | { every: number };

export type AgentDecoratorOptions = {
  name?: string;
  mode?: AgentMode;
  mount?: string;
  cors?: string[];
  auth?: boolean;
  webhookSuffix?: string;
  snapshotting?: SnapshottingOption;
  phantom?: boolean;
};

function parseDurationToNanoseconds(duration: string): bigint {
  const milliseconds = ms(duration as ms.StringValue);
  if (milliseconds === undefined) {
    throw new Error(
      `Invalid duration string: '${duration}'. Use formats like '5s', '10m', '1h', '2 days', etc.`,
    );
  }
  return BigInt(milliseconds) * 1_000_000n;
}

function resolveSnapshotting(option?: SnapshottingOption): Snapshotting {
  if (option === undefined || option === 'disabled') {
    return { tag: 'disabled' };
  }
  if (option === 'enabled') {
    return { tag: 'enabled', val: { tag: 'default' } };
  }
  if ('periodic' in option) {
    return {
      tag: 'enabled',
      val: { tag: 'periodic', val: parseDurationToNanoseconds(option.periodic) },
    };
  }
  return { tag: 'enabled', val: { tag: 'every-n-invocation', val: option.every } };
}

/**
 *
 * The `@agent()` decorator: Marks a class as an Agent, and registers itself internally for discovery by other agents.
 * The agent-name is the class name by default, but can be overridden by passing a custom name to the decorator.
 *
 * It also adds a static `get()` method to the class, which can be used to create a remote client for the agent.
 *
 * Only a class that extends `BaseAgent` can be decorated with `@agent()`.
 *
 * ### Naming of agents
 * By default, the agent name is the class name. When using the agent through
 * Golem's CLI, these names must be provided in kebab-case.
 *
 * Example:
 * ```ts
 * @agent()
 * class WeatherAgent {} // -> "weather-agent"
 * ```
 * You can override the name using the `name` option.
 *
 * ### Durability mode
 * By default, agents are durable. You can specify the durability mode using the optional parameter:
 * ```ts
 * @agent({ mode: "ephemeral" })
 * class EphemeralAgent extends BaseAgent { ... }
 * ```
 * Valid modes are "durable" (default) and "ephemeral".
 *
 * ### Options
 * The decorator accepts an optional configuration object with the following fields:
 * - `name`: Custom agent name (default: class name)
 * - `mode`: Agent durability mode, either "durable" or "ephemeral" (default: "durable")
 *
 * ### HTTP Mount Options
 * Agents can optionally expose an HTTP API using a base mount path. The following options are available:
 * - `mount`: The base HTTP path to expose agent methods (e.g., `'/api/weather'`). A path can have path variables (example: `'/api/{city}/weather'`),
 *    or system variables (e.g., `'/api/{agent-type}/status'`) or both.
 * - `headers`: Default HTTP headers mapped to constructor parameters (e.g., `{ 'X-Api-Key': 'apiKey' }`).
 *    Note that the value of the header is the name of the constructor parameter to which it maps. In this case `apiKey` is one of the constructor parameters.
 * -  Note that all the parameters in the constructor must be provided either via header variables or path variables.
 * - `auth`: Boolean flag indicating if authentication is required for all HTTP endpoints.
 * - `cors`: Array of allowed origins for cross-origin requests (e.g., `['https://app.acme.com']` or `['*']` for all origins).
 * - `webhookSuffix`: Optional suffix path that gets appended to the globally configured webhook url exposing webhook endpoints
 * - Only if we have a mount defined, we can use `endpoint` decorator on methods to expose them over HTTP.
 *
 *
 * Example with HTTP mount:
 * ```ts
 * @agent({
 *   mount: '/api/weather',
 *   headers: { 'X-Api-Key': 'apiKey' },
 *   auth: true,
 *   cors: ['https://app.acme.com'],
 *   webhookSuffix: '/webhook/{event}'
 * })
 * class WeatherAgent {
 *   constructor(apiKey: string) {}
 *
 *   @endpoint({ get: '/current/{city}' })
 *   getWeather(city: string): WeatherReport { ... }
 * }
 * ```
 *
 * ### Metadata
 * Prompt and description are **recommended** so that other agents can decide whether to interact with this agent.
 * ```ts
 * @prompt("Provide a city name")
 * @description("Get the current weather for a location")
 * getWeather(city: string): Promise<WeatherResult> { ... }
 * ```
 *
 * ### Agent parameter types
 *
 * - Constructor and method parameters can be any valid TypeScript type.
 * - **Enums are not supported**.
 * - Use **type aliases** for clarity and reusability.
 *
 * ```ts
 * type Coordinates = { lat: number; lon: number };
 * type WeatherReport = { temperature: number; description: string };
 *
 * @agent()
 * class WeatherAgent {
 *   constructor(apiKey: string) {}
 *
 *   getWeather(coords: Coordinates): WeatherReport { ... }
 * }
 * ```
 *
 * ### Example
 *
 * ```ts
 * @agent()
 * class CalculatorAgent {
 *   constructor(baseValue: number) {}
 *
 *   add(value: number): number {
 *     return this.baseValue + value;
 *   }
 * }
 *
 * ### Remote Client
 *
 * The purpose of a remote client is that it allows you to invoke the agent constructor
 * and methods of an agent (even if it's defined within the same code) in a different container.
 *
 * By passing the constructor parameters to `get()`, the SDK will ensure that an instance of the agent,
 * is created in a different container, and the method calls are proxied to that container.
 *
 * `get` takes the same parameters as the constructor.
 *
 * The main difference between `CalculatorAgent.get(10)` and `new CalculatorAgent(10)` is that
 * the former creates or fetches a remote instance of the agent in a different container, while the latter creates a local instance.
 * If the remote agent was already created with the same constructor parameter value, `get` will return a reference to the existing agent instead of creating a new one.

 * ```ts
 * const calcRemote = CalculatorAgent.get(10);
 * calcRemote.add(5);
 * ```
 *
 * It is possible to create remote clients to phantom agents - agents sharing the same constructor values as a normal agent,
 * but still having their separate identity. To address a phantom agent, use the `phantom` method instead of `get`:
 *
 * ```ts
 * const phantomRemote = CalculatorAgent.phantom(undefined, 10);
 * phantomRemote.add(5);
 *
 * // or
 *
 * const phantomRemote = CalculatorAgent.phantom(parseUuid("A09F61A8-677A-40EA-9EBE-437A0DF51749"), 10);
 * phantomRemote.add(5);
 * ```
 *
 * The first parameter is the phantom ID. If undefined, a new phantom ID will be generated.
 */
export function agent(options?: AgentDecoratorOptions) {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any -- any[] required for legacy decorator contravariance
  return function <T extends new (...args: any[]) => BaseAgent>(ctor: T) {
    if (!Object.prototype.isPrototypeOf.call(BaseAgent, ctor)) {
      throw new Error(
        `Invalid agent declaration: \`${ctor.name}\` must extend \`BaseAgent\` to be decorated with @agent()`,
      );
    }

    const agentClassName = new AgentClassName(ctor.name);

    if (AgentTypeRegistry.exists(agentClassName)) {
      return ctor;
    }

    const classMetadata = TypeMetadata.get(ctor.name);

    if (!classMetadata) {
      const availableAgents = Array.from(TypeMetadata.getAll().entries())
        .map(([key, _]) => key)
        .join(', ');

      throw new Error(
        `Agent class ${agentClassName.value} is not registered. Available agents are ${availableAgents}. Please ensure the class ${ctor.name} is decorated with @agent()`,
      );
    }

    const constructorDataSchema = getAgentConstructorSchema(agentClassName.value, classMetadata);

    const httpMount = getHttpMountDetails(options);

    const methods = getAgentMethodSchema(classMetadata, agentClassName.value, httpMount);

    const agentTypeName = new AgentClassName(options?.name || agentClassName.value);

    if (AgentInitiatorRegistry.exists(agentTypeName.value)) {
      throw new Error(
        `Agent name conflict: Another agent with the name "${agentTypeName}" is already registered. Please choose a different agent name for the class \`${agentClassName.value}\` using \`@agent("custom-name")\``,
      );
    }

    const agentTypeDescription =
      AgentConstructorRegistry.lookup(agentClassName.value)?.description ??
      `Constructs the agent ${agentTypeName.value}`;

    const constructorParameterNames = classMetadata.constructorArgs
      .map((arg) => arg.name)
      .join(', ');

    const defaultPromptHint = `Enter the following parameters: ${constructorParameterNames}`;

    const agentTypePromptHint =
      AgentConstructorRegistry.lookup(agentClassName.value)?.prompt ?? defaultPromptHint;

    const constructor: AgentConstructor = {
      name: agentClassName.value,
      description: agentTypeDescription,
      promptHint: agentTypePromptHint,
      inputSchema: constructorDataSchema,
    };

    if (httpMount) {
      validateHttpMount(agentClassName.value, httpMount, constructor);
    }

    const agentType: AgentType = {
      typeName: agentTypeName.value,
      description: agentTypeDescription,
      constructor,
      methods,
      dependencies: [],
      mode: options?.mode ?? 'durable',
      httpMount,
      snapshotting: resolveSnapshotting(options?.snapshotting),
      config: [],
    };

    AgentTypeRegistry.register(agentClassName, agentType);

    const constructorParamTypes: ParameterDetail[] | undefined = TypeMetadata.get(
      agentClassName.value,
    )?.constructorArgs.map((arg) => {
      const typeInfo = AgentConstructorParamRegistry.getParamType(agentClassName.value, arg.name);

      if (!typeInfo) {
        throw new Error(
          `Unsupported type for constructor parameter ${arg.name} in agent ${agentClassName}`,
        );
      }

      return { name: arg.name, type: typeInfo };
    });

    if (!constructorParamTypes) {
      throw new Error(
        `Failed to retrieve constructor parameter types for agent ${agentClassName.value}.`,
      );
    }

    const c = ctor as unknown as Record<string, unknown>;
    c.get = getRemoteClient(agentClassName, agentType, ctor);
    c.newPhantom = getNewPhantomRemoteClient(agentClassName, agentType, ctor);

    c.getPhantom = getPhantomRemoteClient(agentClassName, agentType, ctor);

    AgentInitiatorRegistry.register(agentTypeName, {
      initiate: (constructorInput: DataValue, principal: Principal) => {
        const deserializedConstructorArgs = deserializeDataValue(
          constructorInput,
          constructorParamTypes,
          principal,
        );

        if (Either.isLeft(deserializedConstructorArgs)) {
          const error = createCustomError(
            `Failed to deserialize constructor arguments for agent ${agentClassName.value}: ${deserializedConstructorArgs.val}`,
          );

          return {
            tag: 'err',
            val: error,
          };
        }

        const instance = new ctor(...deserializedConstructorArgs.val);

        const agentId = getRawSelfAgentId();
        if (!agentId.value.startsWith(agentTypeName.asWit)) {
          const error = createCustomError(
            `Expected the container name in which the agent is initiated to start with "${agentTypeName.asWit}", got "${agentId.value}"`,
          );

          return {
            tag: 'err',
            val: error,
          };
        }

        instance.getId = () => agentId;

        const resolvedAgent = new ResolvedAgent(
          instance,
          agentClassName,
          agentId,
          constructorInput,
        );

        return {
          tag: 'ok',
          val: resolvedAgent,
        };
      },
    });
  };
}
