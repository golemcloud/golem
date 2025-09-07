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

import { AgentType, DataValue, AgentError } from 'golem:agent/common';
import { AgentInternal } from './internal/agentInternal';
import { ResolvedAgent } from './internal/resolvedAgent';
import { MethodParams, TypeMetadata } from '@golemcloud/golem-ts-types-core';
import { Type } from '@golemcloud/golem-ts-types-core';
import { getRemoteClient } from './internal/clientGeneration';
import { BaseAgent } from './baseAgent';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
import * as WitValue from './internal/mapping/values/WitValue';
import * as Either from './newTypes/either';
import {
  getAgentMethodSchema,
  getConstructorDataSchema,
} from './internal/schema';
import * as Option from './newTypes/option';
import { AgentMethodMetadataRegistry } from './internal/registry/agentMethodMetadataRegistry';
import { AgentClassName } from './newTypes/agentClassName';
import { AgentInitiatorRegistry } from './internal/registry/agentInitiatorRegistry';
import { getSelfMetadata } from 'golem:api/host@1.1.7';
import { AgentId } from './agentId';
import { createCustomError } from './internal/agentError';
import { AgentTypeName } from './newTypes/agentTypeName';

type Type = Type.Type;
/**
 * Marks a class as an Agent and registers it in the global agent registry.
 * Note that the method generates a `local` and `remote` client for the agent.
 * The details of these clients are explained further below.
 *
 * The `@agent()` decorator:
 * - Registers the agent type for discovery by other agents.
 * - Inspects the constructor to determine its parameter types.
 * - Inspects all methods to determine their input/output parameter types.
 * - Associates metadata such as `prompt` and `description` with the agent.
 * - Creates `.createLocal()` and `.createRemote()` factory methods on the class.
 * - Enables schema-based validation of parameters and return values.
 *
 * ### Naming
 * By default, the agent name is the kebab-case of the class name.
 * Example:
 * ```ts
 * @agent()
 * class WeatherAgent {} // -> "weather-agent"
 * ```
 * You can override the name using explicit metadata.
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
 * Please note that there are a few limitations in what can be types of these parameters.
 * Please read through the documentation that list the types that are currently supported.
 *
 * - Constructor and method parameters can be any valid TypeScript type.
 * - **Enums are not supported**.
 * - Use **type aliases** for clarity and reusability.
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
 * ### Remote and Local Clients
 *
 * A local client is a direct instance of the agent class,
 * which can be used to call methods directly. It is recommended to use the local clients
 * even if you can create a local client by directly calling the constructor.
 *
 * With a local client, any logic defined in the agent class is executed in the same container.
 *
 * const calc = CalculatorAgent.createLocal(10);
 * console.log(calc.add(5)); // 15
 *
 * The purpose of a remote client is that it allows you to invoke the agent constructor
 * and methods of an agent (even if it's defined with in the same code) in a different container.
 * An immediate outcome of this is that you are offloading the work of this agent to a different container
 * than the current container.
 *
 * const calcRemote = CalculatorAgent.createRemote();
 * calcRemote.add(5);
 * ```
 */
export function agent() {
  return function <T extends new (...args: any[]) => any>(ctor: T) {
    const agentClassName = new AgentClassName(ctor.name);

    if (AgentTypeRegistry.exists(agentClassName)) {
      return ctor;
    }

    let classMetadata = Option.getOrElse(
      Option.fromNullable(TypeMetadata.get(ctor.name)),
      () => {
        const availableAgents = Array.from(TypeMetadata.getAll().entries())
          .map(([key, _]) => key)
          .join(', ');
        throw new Error(
          `Agent class ${agentClassName.value} is not registered in TypeMetadata. Available agents are ${availableAgents}. Please ensure the class ${ctor.name} is decorated with @agent()`,
        );
      },
    );

    const constructorDataSchema = Either.getOrElse(
      getConstructorDataSchema(classMetadata),
      (err) => {
        throw new Error(
          `Schema generation failed for agent class ${agentClassName.value} due to unsupported types in constructor. ` +
            err,
        );
      },
    );

    const methodSchemaEither = getAgentMethodSchema(
      classMetadata,
      agentClassName,
    );

    // Note: Either.getOrThrowWith doesn't seem to work within the decorator context
    if (Either.isLeft(methodSchemaEither)) {
      throw new Error(
        `Schema generation failed for agent class ${agentClassName.value}. ${methodSchemaEither.val}`,
      );
    }

    const methods = methodSchemaEither.val;

    const agentTypeName = AgentTypeName.fromAgentClassName(agentClassName);

    const agentType: AgentType = {
      typeName: agentTypeName.value,
      description: agentClassName.value,
      constructor: {
        name: agentClassName.value,
        description: `Constructs ${agentClassName}`,
        promptHint: 'Enter something...',
        inputSchema: constructorDataSchema,
      },
      methods,
      dependencies: [],
    };

    AgentTypeRegistry.register(agentClassName, agentType);

    (ctor as any).get = getRemoteClient(ctor);

    AgentInitiatorRegistry.register(
      AgentTypeName.fromAgentClassName(agentClassName),
      {
        initiate: (agentName: string, constructorParams: DataValue) => {
          const constructorInfo = classMetadata.constructorArgs;

          const constructorParamTypes: Type[] = constructorInfo.map(
            (p) => p.type,
          );

          const constructorParamWitValues =
            getWitValueFromDataValue(constructorParams);

          const convertedConstructorArgs = constructorParamWitValues.map(
            (witVal, idx) => {
              return WitValue.toTsValue(witVal, constructorParamTypes[idx]);
            },
          );

          const instance = new ctor(...convertedConstructorArgs);

          const containerName = getSelfMetadata().workerId.workerName;

          if (!containerName.startsWith(agentName)) {
            const error = createCustomError(
              `Expected the container name in which the agent is initiated to start with "${agentName}", but got "${containerName}"`,
            );

            return {
              tag: 'err',
              val: error,
            };
          }

          // When an agent is initiated using an initializer,
          // it runs in a worker, and the name of the worker is in-fact the agent-id
          // Example: weather-agent-{"US", celsius}
          const uniqueAgentId = new AgentId(containerName);

          (instance as BaseAgent).getId = () => uniqueAgentId;

          const agentInternal: AgentInternal = {
            getId: () => {
              return uniqueAgentId;
            },
            getAgentType: () => {
              const agentType = AgentTypeRegistry.lookup(agentClassName);

              if (Option.isNone(agentType)) {
                throw new Error(
                  `Failed to find agent type for ${agentClassName}. Ensure it is decorated with @agent() and registered properly.`,
                );
              }

              return agentType.val;
            },
            invoke: async (method, args) => {
              const fn = instance[method];
              if (!fn)
                throw new Error(
                  `Method ${method} not found on agent ${agentClassName}`,
                );

              const agentTypeOpt = AgentTypeRegistry.lookup(agentClassName);

              if (Option.isNone(agentTypeOpt)) {
                const error: AgentError = {
                  tag: 'invalid-method',
                  val: `Agent type ${agentClassName} not found in registry.`,
                };
                return {
                  tag: 'err',
                  val: error,
                };
              }

              const agentType = agentTypeOpt.val;

              const methodInfo = classMetadata.methods.get(method);

              if (!methodInfo) {
                const error: AgentError = {
                  tag: 'invalid-method',
                  val: `Method ${method} not found in metadata for agent ${agentClassName}.`,
                };
                return {
                  tag: 'err',
                  val: error,
                };
              }

              const paramTypes: MethodParams = methodInfo.methodParams;

              const argsWitValues = getWitValueFromDataValue(args);

              const returnType: Type = methodInfo.returnType;

              const paramTypeArray = Array.from(paramTypes.values());

              const convertedArgs = argsWitValues.map((witVal, idx) => {
                const paramType = paramTypeArray[idx];
                return WitValue.toTsValue(witVal, paramType);
              });

              const result = await fn.apply(instance, convertedArgs);

              const methodDef = agentType.methods.find(
                (m) => m.name === method,
              );

              if (!methodDef) {
                const entriesAsStrings = Array.from(
                  AgentTypeRegistry.entries(),
                ).map(
                  ([key, value]) =>
                    `Key: ${key}, Value: ${JSON.stringify(value, null, 2)}`,
                );

                const error: AgentError = {
                  tag: 'invalid-method',
                  val: `Method ${method} not found in agent type ${agentClassName}. Available methods: ${entriesAsStrings.join(
                    ', ',
                  )}`,
                };

                return {
                  tag: 'err',
                  val: error,
                };
              }

              const returnValue = WitValue.fromTsValue(result, returnType);

              if (Either.isLeft(returnValue)) {
                const agentError: AgentError = {
                  tag: 'invalid-method',
                  val: `Invalid return value from ${method}: ${returnValue.val}`,
                };

                return {
                  tag: 'err',
                  val: agentError,
                };
              }

              return {
                tag: 'ok',
                val: getDataValueFromWitValue(returnValue.val),
              };
            },
          };

          return {
            tag: 'ok',
            val: new ResolvedAgent(agentClassName, agentInternal, instance),
          };
        },
      },
    );
  };
}

export function prompt(prompt: string) {
  return function (target: Object, propertyKey: string) {
    const agentClassName = new AgentClassName(target.constructor.name);
    AgentMethodMetadataRegistry.setPromptName(
      agentClassName,
      propertyKey,
      prompt,
    );
  };
}

export function description(desc: string) {
  return function (target: Object, propertyKey: string) {
    const agentClassName = new AgentClassName(target.constructor.name);
    AgentMethodMetadataRegistry.setDescription(
      agentClassName,
      propertyKey,
      desc,
    );
  };
}

// FIXME: in the next version, handle all dataValues
export function getWitValueFromDataValue(
  dataValue: DataValue,
): WitValue.WitValue[] {
  if (dataValue.tag === 'tuple') {
    return dataValue.val.map((elem) => {
      if (elem.tag === 'component-model') {
        return elem.val;
      } else {
        throw new Error(`Unsupported element type: ${elem.tag}`);
      }
    });
  } else {
    throw new Error(`Unsupported DataValue type: ${dataValue.tag}`);
  }
}

// Why is return value a tuple with a single element?
// why should it have a name?
export function getDataValueFromWitValue(
  witValue: WitValue.WitValue,
): DataValue {
  return {
    tag: 'tuple',
    val: [
      {
        tag: 'component-model',
        val: witValue,
      },
    ],
  };
}
