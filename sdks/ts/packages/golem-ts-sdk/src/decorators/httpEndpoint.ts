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
import {
  AuthDetails,
  CorsOptions,
  HeaderVariable,
  HttpEndpointDetails,
  PathSegment,
  QueryVariable,
} from 'golem:agent/common';
import { AgentMethodRegistry } from '../internal/registry/agentMethodRegistry';
import { parsePath } from '../internal/http/path';
import { parseQuery } from '../internal/http/query';

export type EndpointDecoratorOptions = {
  get?: string;
  post?: string;
  put?: string;
  delete?: string;
  custom?: {
    method: string;
    path: string;
  };
  headers?: Record<string, string>;
  auth?: boolean;
  cors?: string[];
};

/**
 * Marks a method as an HTTP-accessible endpoint for an agent.
 *
 * Only methods of classes decorated with `@agent()` and an optional HTTP mount
 * can be annotated with `@endpoint()`. This decorator registers the method as
 * an HTTP endpoint and allows it to receive path variables, header variables,
 * query parameters, and apply authentication and CORS rules.
 *
 * ### Example: Basic GET Endpoint
 * ```ts
 * @agent({ mount: '/api' })
 * class WeatherAgent {
 *   constructor(apiKey: string) {}
 *
 *   @endpoint({ get: '/weather/{city}' })
 *   getWeather(city: string): WeatherReport { ... }
 * }
 * ```
 *
 * ### Example: Multiple Endpoints for a Single Method
 * You can decorate the same method with multiple HTTP endpoints.
 * ```ts
 * @endpoint({ get: '/weather/{city}' })
 * @endpoint({ post: '/weather', headers: { 'X-City': 'city' } })
 * getWeather(city: string): WeatherReport { ... }
 * ```
 *
 * ### HTTP Methods
 * - Specify **one** of `get`, `post`, `put`, `delete`, or `custom`.
 * - The value of the option is the endpoint path.
 * - Examples:
 *   - Simple GET: `{ get: '/status' }`
 *   - GET with path variables: `{ get: '/rooms/{roomId}/messages/{messageId}' }`
 * - Path variables must exactly match method parameters.
 * - No "foreign" path variables are allowed.
 *
 * ### Path Variables
 * - Declared using `{variableName}` in the path.
 * - Must correspond to a method parameter.
 * - Example:
 * ```ts
 * getRoomMessage(@param roomId: string, @param messageId: string)
 * @endpoint({ get: '/rooms/{roomId}/messages/{messageId}' })
 * ```
 *
 * ### Header Variables
 * - Map HTTP headers to method parameters using the `headers` option.
 * - Example:
 * ```ts
 * @endpoint({
 *   post: '/create',
 *   headers: { 'X-Tenant': 'tenantId', 'X-Request-Id': 'requestId' }
 * })
 * createSomething(tenantId: string, requestId: string) { ... }
 * ```
 *
 * ### Query Variables
 * - Defined in the path using standard query syntax `?foo={bar}&limit={count}`.
 * - Must match method parameters exactly.
 * - Example:
 * ```ts
 * @endpoint({ get: '/search?query={q}&limit={n}' })
 * search(q: string, n: number) { ... }
 * ```
 *
 * ### CORS
 * - Use the `cors` option to allow cross-origin requests.
 * - Example: `cors: ['https://app.acme.com']` or `cors: ['*']` to allow all origins.
 *
 * ### Authentication
 * - `auth: true` requires the request to be authenticated.
 * - Example:
 * ```ts
 * @endpoint({ get: '/secure-data', auth: true })
 * getSecureData(userId: string) { ... }
 * ```
 *
 * ### Errors / Validation
 * This decorator will throw an error if:
 * - No HTTP method is specified.
 * - Path does not start with `/`.
 * - Path contains query parameters (`?`) when using the path property.
 * - Path or header variables do not exist on the method parameters.
 *
 * ### Notes
 * - Methods can be decorated multiple times to expose different HTTP routes.
 * - Path, header, and query variables are strictly validated against method parameters.
 * - See `EndpointDecoratorOptions` for all available configuration options.
 */
export function endpoint(opts: EndpointDecoratorOptions) {
  return function (target: Object, propertyKey: string | symbol, descriptor: PropertyDescriptor) {
    const className = target.constructor.name;
    const methodName = String(propertyKey);

    const classMetadata = TypeMetadata.get(className);
    if (!classMetadata) {
      throw new Error(
        `Class metadata not found for agent ${className}. Ensure metadata is generated.`,
      );
    }

    const methods = ['get', 'post', 'put', 'delete', 'custom'] as const;

    const providedMethods = methods.filter((m) => (m === 'custom' ? !!opts.custom : !!opts[m]));

    if (providedMethods.length === 0) {
      throw new Error(
        `Endpoint decorator must specify one HTTP method (get/post/put/delete/custom) for method ${methodName}`,
      );
    }

    if (providedMethods.length > 1) {
      throw new Error(
        `Endpoint decorator must specify only one HTTP method for method ${methodName}. Provided: ${providedMethods.join(', ')}`,
      );
    }

    const selectedMethod = providedMethods[0];

    let httpMethod: HttpEndpointDetails['httpMethod'];

    let pathWithQuery: string;

    if (selectedMethod === 'custom') {
      const custom = opts.custom!;

      if (!custom.method || !custom.path) {
        throw new Error(
          `Custom endpoint must specify both method and path for method ${methodName}`,
        );
      }

      httpMethod = { tag: 'custom', val: custom.method };

      pathWithQuery = custom.path;
    } else {
      httpMethod = { tag: selectedMethod };
      pathWithQuery = opts[selectedMethod]!;
    }

    const pathAndQuery = split_path_and_query(pathWithQuery);

    const pathSuffix: PathSegment[] = parsePath(pathAndQuery.path);

    const queryVars: QueryVariable[] = pathAndQuery.query
      ? parseQuery(pathAndQuery.query) || []
      : [];

    const headerVars: HeaderVariable[] = Object.entries(opts.headers || {}).map(
      ([headerName, variableName]) => ({ headerName, variableName }),
    );

    const authDetails: AuthDetails = { required: opts.auth ?? false };

    const corsOptions: CorsOptions = { allowedPatterns: opts.cors ?? [] };

    const httpEndpoint: HttpEndpointDetails = {
      httpMethod,
      pathSuffix,
      headerVars,
      queryVars,
      authDetails,
      corsOptions,
    };

    AgentMethodRegistry.setHttpEndpoint(className, methodName, httpEndpoint);
  };
}

function split_path_and_query(pathWithQuery: string): {
  path: string;
  query: string | null;
} {
  const [path, query] = pathWithQuery.split('?', 2);
  return { path, query: query ?? null };
}
