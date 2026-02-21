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

      const classMetadata = TypeMetadata.get(className);
      if (!classMetadata) {
        throw new Error(
          `Class metadata not found for agent ${className}. Ensure metadata is generated.`,
        );
      }

      AgentConstructorRegistry.setDescription(className, description);
    } else {
      const className = target.constructor.name;

      const classMetadata = TypeMetadata.get(className);
      if (!classMetadata) {
        throw new Error(
          `Class metadata not found for agent ${className}. Ensure metadata is generated.`,
        );
      }

      const methodName = String(propertyKey);

      AgentMethodRegistry.setDescription(className, methodName, description);
    }
  };
}
