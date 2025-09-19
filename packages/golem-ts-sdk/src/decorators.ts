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
import { AgentMethodRegistry } from './internal/registry/agentMethodRegistry';
import { AgentClassName } from './newTypes/agentClassName';
import { AgentInitiatorRegistry } from './internal/registry/agentInitiatorRegistry';
import { getSelfMetadata } from 'golem:api/host@1.1.7';
import { AgentId } from './agentId';
import { createCustomError } from './internal/agentError';
import { AgentTypeName } from './newTypes/agentTypeName';
import { AgentConstructorParamRegistry } from './internal/registry/agentConstructorParamRegistry';
import { AgentMethodParamRegistry } from './internal/registry/agentMethodParamRegistry';
import { AgentConstructorRegistry } from './internal/registry/agentConstructorRegistry';
import { UnstructuredText } from './newTypes/textInput';
import { UnstructuredBinary } from './newTypes/binaryInput';

type TsType = Type.Type;

/**
 * Marks a class as an Agent and registers it in the global agent registry.
 * Note that the method generates a `local` and `remote` client for the agent.
 * The details of these clients are explained further below.
 *
 * The `@agent()` decorator: Registers the agent type for discovery by other agents.
 *
 * ### Naming
 * By default, the agent name is the kebab-case of the class name.
 *
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
 * ### Remote Client
 *
 * The purpose of a remote client is that it allows you to invoke the agent constructor
 * and methods of an agent (even if it's defined with in the same code) in a different container.
 * By passing the constructor parameters to `get()`, the SDK will ensure that an instance of the agent,
 * is created in a different container, and the method calls are proxied to that container.
 *
 *
 * const calcRemote = CalculatorAgent.get(10);
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
      getConstructorDataSchema(agentClassName, classMetadata),
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

          const constructorParamTypes: TsType[] = constructorInfo.map(
            (p) => p.type,
          );

          const deserializedConstructorArgs = deserializeDataValue(
            constructorParams,
            constructorParamTypes,
          );

          const instance = new ctor(...deserializedConstructorArgs);

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
            getParameters(): DataValue {
              return constructorParams;
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
            loadSnapshot(bytes: Uint8Array): Promise<void> {
              return (instance as BaseAgent).loadSnapshot(bytes);
            },
            saveSnapshot(): Promise<Uint8Array> {
              return (instance as BaseAgent).saveSnapshot();
            },
            invoke: async (methodName, methodArgs) => {
              const fn = instance[methodName];
              if (!fn)
                throw new Error(
                  `Method ${methodName} not found on agent ${agentClassName}`,
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

              const methodInfo = classMetadata.methods.get(methodName);

              if (!methodInfo) {
                const error: AgentError = {
                  tag: 'invalid-method',
                  val: `Method ${methodName} not found in metadata for agent ${agentClassName}.`,
                };
                return {
                  tag: 'err',
                  val: error,
                };
              }

              const paramTypes: MethodParams = methodInfo.methodParams;

              const returnType: TsType = methodInfo.returnType;

              const paramTypeArray = Array.from(paramTypes.values());

              const convertedArgs = deserializeDataValue(
                methodArgs,
                paramTypeArray,
              );

              const result = await fn.apply(instance, convertedArgs);

              const methodDef = agentType.methods.find(
                (m) => m.name === methodName,
              );

              if (!methodDef) {
                const error: AgentError = {
                  tag: 'invalid-method',
                  val: `Method ${methodName} not found in agent type ${agentClassName}`,
                };

                return {
                  tag: 'err',
                  val: error,
                };
              }

              const returnValue = WitValue.fromTsValue(result, returnType);

              if (Either.isLeft(returnValue)) {
                const agentError: AgentError = {
                  tag: 'invalid-type',
                  val: `Failed to serialize the return value from ${methodName}: ${returnValue.val}`,
                };

                return {
                  tag: 'err',
                  val: agentError,
                };
              }

              return {
                tag: 'ok',
                val: getDataValueFromReturnValueWit(returnValue.val),
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

/**
 * Marks a class or method as **multimodal**.
 *
 * Usage:
 *
 * ```ts
 * @multimodal()
 * class ImageTextAgent {
 *   @multimodal()
 *   classify(input: { text: string; image: string }): string {
 *     // ...
 *   }
 * }
 * ```
 */
export function multimodal() {
  return function (
    target: Object | Function,
    propertyKey?: string | symbol,
    descriptor?: PropertyDescriptor,
  ) {
    if (propertyKey === undefined) {
      const className = (target as Function).name;
      const agentClassName = new AgentClassName(className);

      const classMetadata = TypeMetadata.get(agentClassName.value);
      if (!classMetadata) {
        throw new Error(
          `Class metadata not found for agent ${agentClassName}. Ensure metadata is generated.`,
        );
      }

      AgentConstructorRegistry.setAsMultiModal(agentClassName);
    } else {
      const agentClassName = new AgentClassName(target.constructor.name);

      const classMetadata = TypeMetadata.get(agentClassName.value);
      if (!classMetadata) {
        throw new Error(
          `Class metadata not found for agent ${agentClassName}. Ensure metadata is generated.`,
        );
      }

      const methodName = String(propertyKey);

      AgentMethodRegistry.setAsMultimodal(agentClassName, methodName);
    }
  };
}

/*
 * Associates a list of **language codes** with a parameter in either constructor or method.
 * languageCodes is valid only when the type is `UnstructuredText`.
 *
 * Example:
 *
 * ```ts
 * class TextAgent extends BaseAgent {
 *   constructor(@languageCodes(["en", "fr"]) text: UnstructuredText) {}
 *  ..
 * }
 * ```
 *
 * @param codes A list of BCP-47 language codes (e.g., "en", "fr", "es").
 */
export function languageCodes(codes: string[]) {
  return function (
    target: Object,
    propertyKey: string | symbol | undefined, // method name if its part of method or undefined if its constructor
    parameterIndex: number, // parameter index
  ) {
    if (propertyKey === undefined) {
      const agentClassName = new AgentClassName((target as Function).name);

      const classMetadata = TypeMetadata.get(agentClassName.value);

      const constructorInfo = classMetadata?.constructorArgs;

      if (!constructorInfo) {
        throw new Error(
          `Constructor metadata not found for agent ${agentClassName}. Ensure the constructor exists and is not private/protected.`,
        );
      }

      const paramName = constructorInfo[parameterIndex].name;

      AgentConstructorParamRegistry.setLanguageCodes(
        agentClassName,
        paramName,
        codes,
      );
    } else {
      const agentClassName = new AgentClassName(target.constructor.name);

      const classMetadata = TypeMetadata.get(agentClassName.value);

      const methodName = String(propertyKey);

      const methodInfo = classMetadata?.methods.get(methodName);

      if (!methodInfo) {
        throw new Error(
          `Method ${methodName} not found in metadata for agent ${agentClassName}. Ensure the method exists and is not private/protected.`,
        );
      }

      const paramName = Array.from(methodInfo.methodParams).map(
        (paramType) => paramType[0],
      )[parameterIndex];

      // console.log(`applying method param decorator to ${agentClassName.value}, ${methodName}, ${paramName} and ${codes}`)

      AgentMethodParamRegistry.setLanguageCodes(
        agentClassName,
        methodName,
        paramName,
        codes,
      );
    }
  };
}

/*
 * Associates a list of **MIME types** with a parameter in either constructor or method.
 * mimeTypes is valid only when the type is `UnstructuredBinary` or `UnstructuredText`.
 *
 * Example:
 *
 * ```ts
 * class FileAgent extends BaseAgent {
 *   constructor(@mimeTypes(["application/pdf", "image/png"]) fileContent: UnstructuredBinary) {}
 *   ..
 * }
 *
 * ```
 *
 * @param mimeTypes A list of MIME types (e.g., "text/plain", "application/json").
 */
export function mimeTypes(mimeTypes: string[]) {
  return function (
    target: Object,
    propertyKey: string | symbol | undefined, // method name if its part of method or undefined if its constructor
    parameterIndex: number, // parameter index
  ) {
    if (propertyKey === undefined) {
      const agentClassName = new AgentClassName((target as Function).name);

      const classMetadata = TypeMetadata.get(agentClassName.value);

      const constructorInfo = classMetadata?.constructorArgs;

      if (!constructorInfo) {
        throw new Error(
          `Constructor metadata not found for agent ${agentClassName}. Ensure the constructor exists and is not private/protected.`,
        );
      }

      const paramName = constructorInfo[parameterIndex].name;

      AgentConstructorParamRegistry.setMimeTypes(
        agentClassName,
        paramName,
        mimeTypes,
      );
    } else {
      const agentClassName = new AgentClassName(target.constructor.name);

      const classMetadata = TypeMetadata.get(agentClassName.value);

      const methodName = String(propertyKey);

      const methodInfo = classMetadata?.methods.get(methodName);

      if (!methodInfo) {
        throw new Error(
          `Method ${methodName} not found in metadata for agent ${agentClassName}. Ensure the method exists and is not private/protected.`,
        );
      }

      const paramName = Array.from(methodInfo.methodParams).map(
        (paramType) => paramType[0],
      )[parameterIndex];

      AgentMethodParamRegistry.setMimeTypes(
        agentClassName,
        methodName,
        paramName,
        mimeTypes,
      );
    }
  };
}

/**
 * Associates a **prompt** with a method of an agent.
 *
 * A prompt is valid only for classes that are decorated with `@agent()`.
 *
 * Example:
 * ```ts
 * @agent()
 * class WeatherAgent {
 *   @prompt("Provide a city name")
 *   getWeather(city: string): WeatherReport { ... }
 * }
 * ```
 *
 * @param prompt  A hint that describes what kind of input the agentic method expects.
 * They are especially useful for guiding other agents when deciding how to call this method.
 */
export function prompt(prompt: string) {
  return function (target: Object, propertyKey: string) {
    const agentClassName = new AgentClassName(target.constructor.name);
    AgentMethodRegistry.setPromptName(agentClassName, propertyKey, prompt);
  };
}

/**
 * Associates a **description** with a method of an agent.

 * `@description` is valid only for classes that are decorated with `@agent()`.
 *
 * Example:
 * ```ts
 * @agent()
 * class WeatherAgent {
 *   @description("Get the current weather for a location")
 *   getWeather(city: string): WeatherReport { ... }
 * }
 * ```
 * @param description  A human-readable description of what the method does.
 */
export function description(description: string) {
  return function (target: Object, propertyKey: string) {
    const agentClassName = new AgentClassName(target.constructor.name);
    AgentMethodRegistry.setDescription(
      agentClassName,
      propertyKey,
      description,
    );
  };
}

export function deserializeDataValue(
  dataValue: DataValue,
  paramTypes: TsType[],
): any[] {
  switch (dataValue.tag) {
    case 'tuple':
      const elements = dataValue.val;

      return elements.map((elem, idx) => {
        switch (elem.tag) {
          case 'unstructured-text':
            const textRef = elem.val;
            return UnstructuredText.fromDataValue(textRef);

          case 'unstructured-binary':
            const binaryRef = elem.val;
            return UnstructuredBinary.fromDataValue(binaryRef);

          case 'component-model':
            const witValue = elem.val;
            return WitValue.toTsValue(witValue, paramTypes[idx]);
        }
      });

    case 'multimodal':
      return [];
  }
}

// Why is return value a tuple with a single element?
// why should it have a name?
export function getDataValueFromReturnValueWit(
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
