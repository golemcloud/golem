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

import { TypeMetadata } from '@golemcloud/golem-ts-types-core';
import { AgentConstructorRegistry } from '../internal/registry/agentConstructorRegistry';
import { AgentMethodRegistry } from '../internal/registry/agentMethodRegistry';

/*
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

      const classMetadata = TypeMetadata.get(className);
      if (!classMetadata) {
        throw new Error(
          `Class metadata not found for agent ${className}. Ensure metadata is generated.`,
        );
      }

      AgentConstructorRegistry.setPrompt(className, prompt);
    } else {
      const className = target.constructor.name;

      const classMetadata = TypeMetadata.get(className);
      if (!classMetadata) {
        throw new Error(
          `Class metadata not found for agent ${className}. Ensure metadata is generated.`,
        );
      }

      const methodName = String(propertyKey);

      AgentMethodRegistry.setPrompt(className, methodName, prompt);
    }
  };
}
