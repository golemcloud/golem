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
import { TypeMetadata, Type } from '@golemcloud/golem-ts-types-core';
import { getRemoteClient } from './internal/clientGeneration';
import { BaseAgent } from './baseAgent';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
import * as WitValue from './internal/mapping/values/WitValue';
import * as Either from './newTypes/either';
import {
  getAgentMethodSchema,
  getConstructorDataSchema,
  getLanguageCodes,
  getMimeTypes,
} from './internal/schema';
import * as Option from './newTypes/option';
import { AgentMethodRegistry } from './internal/registry/agentMethodRegistry';
import { AgentClassName } from './newTypes/agentClassName';
import { AgentInitiatorRegistry } from './internal/registry/agentInitiatorRegistry';
import { getSelfMetadata } from 'golem:api/host@1.1.7';
import { AgentId } from './agentId';
import { createCustomError } from './internal/agentError';
import { AgentConstructorParamRegistry } from './internal/registry/agentConstructorParamRegistry';
import { AgentMethodParamRegistry } from './internal/registry/agentMethodParamRegistry';
import { AgentConstructorRegistry } from './internal/registry/agentConstructorRegistry';
import { UnstructuredText } from './newTypes/textInput';
import { UnstructuredBinary } from './newTypes/binaryInput';
import * as Value from './internal/mapping/values/Value';
import * as util from 'node:util';
import { Result } from 'golem:rpc/types@0.2.2';
import { TypeInfoInternal } from './internal/registry/typeInfoInternal';
import {
  convertBinaryReferenceToElementValue,
  convertTextReferenceToElementValue,
} from './internal/mapping/values/elementValue';

/**
 *
 * The `@agent()` decorator: Marks a class as an Agent, and registers itself internally for discovery by other agents.
 * The agent-anem is derived from the class name in kebab-case by default, but can be overridden by passing a custom name to the decorator.
 *
 * It also adds a static `get()` method to the class, which can be used to create a remote client for the agent.
 *
 * Only a class that extends `BaseAgent` can be decorated with `@agent()`.
 *
 * ### Naming of agents
 * By default, the agent name is the class name. When using the agent through
 * Golem's CLI, these names are converted to kebab-case.
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
 * and methods of an agent (even if it's defined with in the same code) in a different container.
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
 */
export function agent(customName?: string) {
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

    const agentTypeName = new AgentClassName(
      customName || agentClassName.value,
    );

    if (AgentInitiatorRegistry.exists(agentTypeName.value)) {
      throw new Error(
        `Agent name conflict: Another agent with the name "${agentTypeName}" is already registered. Please choose a different agent name for the class \`${agentClassName.value}\` using \`@agent("custom-name")\``,
      );
    }

    const agentTypeDescription =
      AgentConstructorRegistry.lookup(agentClassName)?.description ??
      `Constructs the agent ${agentTypeName}`;

    const constructorParameterNames = classMetadata.constructorArgs
      .map((arg) => arg.name)
      .join(', ');

    const defaultPromptHint = `Enter the following parameters: ${constructorParameterNames}`;

    const agentTypePromptHint =
      AgentConstructorRegistry.lookup(agentClassName)?.prompt ??
      defaultPromptHint;

    const agentType: AgentType = {
      typeName: agentTypeName.value,
      description: agentTypeDescription,
      constructor: {
        name: agentClassName.value,
        description: agentTypeDescription,
        promptHint: agentTypePromptHint,
        inputSchema: constructorDataSchema,
      },
      methods,
      dependencies: [],
    };

    AgentTypeRegistry.register(agentClassName, agentType);

    const constructorParamTypes:
      | [string, [Type.Type, TypeInfoInternal]][]
      | undefined = TypeMetadata.get(agentClassName.value)?.constructorArgs.map(
      (arg) => {
        const typeInfo = AgentConstructorParamRegistry.getParamType(
          agentClassName,
          arg.name,
        );

        if (!typeInfo) {
          throw new Error(
            `Unsupported type for constructor parameter ${arg.name} in agent ${agentClassName}`,
          );
        }

        return [arg.name, [arg.type, typeInfo]];
      },
    );

    if (!constructorParamTypes) {
      throw new Error(
        `Failed to retrieve constructor parameter types for agent ${agentClassName.value}.`,
      );
    }

    (ctor as any).get = getRemoteClient(ctor);

    AgentInitiatorRegistry.register(agentTypeName, {
      initiate: (constructorInput: DataValue) => {
        const deserializedConstructorArgs = deserializeDataValue(
          constructorInput,
          constructorParamTypes,
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

        const containerName = getSelfMetadata().workerId.workerName;

        if (!containerName.startsWith(agentTypeName.asWit)) {
          const error = createCustomError(
            `Expected the container name in which the agent is initiated to start with "${agentTypeName.asWit}", but got "${containerName}"`,
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

        const agentInternal = getAgentInternal(
          instance,
          agentClassName,
          uniqueAgentId,
          constructorInput,
        );

        return {
          tag: 'ok',
          val: new ResolvedAgent(agentClassName, agentInternal, instance),
        };
      },
    });
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
 *   process(query: [string], image: [string]): string {
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

/**
 * Associates a **prompt** with a method or constructor of an agent
 *
 * A prompt is valid only for classes that are decorated with `@agent()`.
 * A prompt can be specified either at the class level or method level, or both.
 *
 * Example of prompt at constructor (class) level and method level
 *
 * ```ts
 * @agent()
 * @prompt("Provide an API key for the weather service")
 * class WeatherAgent {
 *   @prompt("Provide a city name")
 *   getWeather(city: string): WeatherReport { ... }
 * }
 * ```
 *
 *
 * @param prompt  A hint that describes what kind of input the agentic method expects.
 * They are especially useful for guiding other agents when deciding how to call this method.
 */
export function prompt(prompt: string) {
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

      AgentConstructorRegistry.setPrompt(agentClassName, prompt);
    } else {
      const agentClassName = new AgentClassName(target.constructor.name);

      const classMetadata = TypeMetadata.get(agentClassName.value);
      if (!classMetadata) {
        throw new Error(
          `Class metadata not found for agent ${agentClassName}. Ensure metadata is generated.`,
        );
      }

      const methodName = String(propertyKey);

      AgentMethodRegistry.setPrompt(agentClassName, methodName, prompt);
    }
  };
}

/**
 * Associates a **description** with a method or constructor of an agent.

 * A `description` is valid only for classes that are decorated with `@agent()`.
 * A `description` can be specified either at the class level or method level, or both.
 *
 * Example:
 * ```ts
 * @agent()
 * @description("An agent that provides weather information")
 * class WeatherAgent {
 *   @description("Get the current weather for a location")
 *   getWeather(city: string): WeatherReport { ... }
 * }
 * ```
 * @param description The details of what exactly the method does.
 */
export function description(description: string) {
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

      AgentConstructorRegistry.setDescription(agentClassName, description);
    } else {
      const agentClassName = new AgentClassName(target.constructor.name);

      const classMetadata = TypeMetadata.get(agentClassName.value);
      if (!classMetadata) {
        throw new Error(
          `Class metadata not found for agent ${agentClassName}. Ensure metadata is generated.`,
        );
      }

      const methodName = String(propertyKey);

      AgentMethodRegistry.setDescription(
        agentClassName,
        methodName,
        description,
      );
    }
  };
}

export function deserializeDataValue(
  dataValue: DataValue,
  paramTypes: [string, [Type.Type, TypeInfoInternal]][],
): Either.Either<any[], string> {
  switch (dataValue.tag) {
    case 'tuple':
      const elements = dataValue.val;

      return Either.all(
        elements.map((elem, idx) => {
          switch (elem.tag) {
            case 'unstructured-text':
              const [type] = paramTypes[idx][1];

              const unstructuredTextParamName = paramTypes[idx][0];

              const textRef = elem.val;

              const languageCodes: Either.Either<string[], string> =
                getLanguageCodes(type);

              if (Either.isLeft(languageCodes)) {
                throw new Error(
                  `Failed to get language codes for parameter ${paramTypes[idx][0]}: ${languageCodes.val}`,
                );
              }

              return UnstructuredText.fromDataValue(
                unstructuredTextParamName,
                textRef,
                languageCodes.val,
              );

            case 'unstructured-binary':
              const unstructuredBinaryParamName = paramTypes[idx][0];

              const [binaryType] = paramTypes[idx][1];

              const binaryRef = elem.val;

              const mimeTypes: Either.Either<string[], string> =
                getMimeTypes(binaryType);

              if (Either.isLeft(mimeTypes)) {
                throw new Error(
                  `Failed to get mime types for parameter ${paramTypes[idx][0]}: ${mimeTypes.val}`,
                );
              }

              return UnstructuredBinary.fromDataValue(
                unstructuredBinaryParamName,
                binaryRef,
                mimeTypes.val,
              );

            case 'component-model':
              const paramType = paramTypes[idx][1];
              const typeInfo = paramType[1];

              if (typeInfo.tag !== 'analysed') {
                throw new Error(
                  `Internal error: Unknown parameter type for ${util.format(Value.fromWitValue(elem.val))} at index ${idx}`,
                );
              }

              const witValue = elem.val;
              return Either.right(WitValue.toTsValue(witValue, typeInfo.val));
          }
        }),
      );

    case 'multimodal':
      const multiModalElements = dataValue.val;

      return Either.all(
        multiModalElements.map(([name, elem]) => {
          switch (elem.tag) {
            case 'unstructured-text':
              const nameAndType = paramTypes.find(
                ([paramName]) => paramName === name,
              );

              if (!nameAndType) {
                throw new Error(
                  `Unable to process multimodal input of elem ${util.format(elem.val)}. Unknown parameter \`${name}\` in multimodal input. Available: ${paramTypes.map((p) => JSON.stringify(p)).join(', ')}`,
                );
              }

              const [type] = nameAndType[1];

              const textRef = elem.val;

              const languageCodes: Either.Either<string[], string> =
                getLanguageCodes(type);

              if (Either.isLeft(languageCodes)) {
                throw new Error(
                  `Failed to get language codes for parameter ${name}: ${languageCodes.val}`,
                );
              }

              return UnstructuredText.fromDataValue(
                name,
                textRef,
                languageCodes.val,
              );

            case 'unstructured-binary':
              const binaryTypeWithName = paramTypes.find(
                ([paramName]) => paramName === name,
              );

              if (!binaryTypeWithName) {
                throw new Error(
                  `Unable to process multimodal input of elem ${util.format(elem.val)}. Unknown parameter \`${name}\` in multimodal input. Available: ${paramTypes.map((p) => JSON.stringify(p)).join(', ')}`,
                );
              }

              const [binaryType] = binaryTypeWithName[1];

              const binaryRef = elem.val;

              const mimeTypes = getMimeTypes(binaryType);

              if (Either.isLeft(mimeTypes)) {
                throw new Error(
                  `Failed to get mime types for parameter ${name}: ${mimeTypes.val}`,
                );
              }

              return UnstructuredBinary.fromDataValue(
                name,
                binaryRef,
                mimeTypes.val,
              );

            case 'component-model':
              const witValue = elem.val;

              const param = paramTypes.find(
                ([paramName]) => paramName === name,
              );

              if (!param) {
                throw new Error(
                  `Unable to process multimodal input of elem ${util.format(Value.fromWitValue(elem.val))}. Unknown parameter \`${name}\` in multimodal input. Available: ${paramTypes.map((p) => JSON.stringify(p)).join(', ')}`,
                );
              }

              const paramType = param[1];
              const typeInfo = paramType[1];

              if (typeInfo.tag !== 'analysed') {
                throw new Error(
                  `Internal error: Unknown parameter type for multimodal input ${util.format(Value.fromWitValue(elem.val))} with name ${name}`,
                );
              }

              return Either.right(WitValue.toTsValue(witValue, typeInfo.val));
          }
        }),
      );
  }
}

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

function getAgentInternal(
  agentInstance: any,
  agentClassName: AgentClassName,
  uniqueAgentId: AgentId,
  constructorInput: DataValue,
): AgentInternal {
  return {
    getId: () => {
      return uniqueAgentId;
    },
    getParameters(): DataValue {
      return constructorInput;
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
      return (agentInstance as BaseAgent).loadSnapshot(bytes);
    },
    saveSnapshot(): Promise<Uint8Array> {
      return (agentInstance as BaseAgent).saveSnapshot();
    },
    invoke: async (methodName, methodArgs) => {
      const agentMethod = agentInstance[methodName];

      if (!agentMethod)
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

      const methodParams = TypeMetadata.get(agentClassName.value)?.methods.get(
        methodName,
      )?.methodParams;

      if (!methodParams) {
        const error: AgentError = {
          tag: 'invalid-method',
          val: `Failed to retrieve parameter types for method ${methodName} in agent ${agentClassName}.`,
        };
        return {
          tag: 'err',
          val: error,
        };
      }

      const methodParamTypes = Array.from(methodParams.entries()).map(
        (param) => {
          const paramName = param[0];
          const paramType = param[1];

          const paramTypeInfo = AgentMethodParamRegistry.getParamType(
            agentClassName,
            methodName,
            paramName,
          );

          if (!paramTypeInfo) {
            throw new Error(
              `Unsupported type for parameter ${paramName} in method ${methodName} of agent ${agentClassName}`,
            );
          }

          return [paramName, [paramType, paramTypeInfo]] as [
            string,
            [Type.Type, TypeInfoInternal],
          ];
        },
      );

      const deserializedArgs: Either.Either<any[], string> =
        deserializeDataValue(methodArgs, methodParamTypes);

      if (Either.isLeft(deserializedArgs)) {
        const error: AgentError = {
          tag: 'invalid-input',
          val: `Failed to deserialize arguments for method ${methodName} in agent ${agentClassName.value}: ${deserializedArgs.val}`,
        };

        return {
          tag: 'err',
          val: error,
        };
      }

      const methodResult = await agentMethod.apply(
        agentInstance,
        deserializedArgs.val,
      );

      const methodSignature = agentTypeOpt.val.methods.find(
        (m) => m.name === methodName,
      );

      if (!methodSignature) {
        const error: AgentError = {
          tag: 'invalid-method',
          val: `Method ${methodName} not found in agent type ${agentClassName}`,
        };

        return {
          tag: 'err',
          val: error,
        };
      }

      const returnTypeAnalysed = AgentMethodRegistry.lookupReturnType(
        agentClassName,
        methodName,
      );

      if (!returnTypeAnalysed) {
        const error: AgentError = {
          tag: 'invalid-type',
          val: `Return type of method ${methodName} in agent ${agentClassName} is not supported. Only primitive types, arrays, objects, and tagged unions (Result types) are supported.`,
        };

        return {
          tag: 'err',
          val: error,
        };
      }

      switch (returnTypeAnalysed.tag) {
        case 'analysed':
          const returnValue = WitValue.fromTsValue(
            methodResult,
            returnTypeAnalysed.val,
          );

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
        case 'unstructured-text':
          const unstructuredText =
            convertTextReferenceToElementValue(methodResult);

          const unstructuredTextValue: DataValue = {
            tag: 'tuple',
            val: [unstructuredText],
          };

          const dataValueResultText: Result<DataValue, AgentError> = {
            tag: 'ok',
            val: unstructuredTextValue,
          };

          return dataValueResultText;

        case 'unstructured-binary':
          const unstructuredBinary =
            convertBinaryReferenceToElementValue(methodResult);

          const unstructuredBinaryValue: DataValue = {
            tag: 'tuple',
            val: [unstructuredBinary],
          };

          const dataValueResult: Result<DataValue, AgentError> = {
            tag: 'ok',
            val: unstructuredBinaryValue,
          };

          return dataValueResult;
      }
    },
  };
}
