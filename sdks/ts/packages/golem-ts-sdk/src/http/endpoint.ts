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
import { parsePath } from './path';
import { parseQuery } from './query';
import { AgentMethodRegistry } from '../internal/registry/agentMethodRegistry';

export type EndpointDecoratorOptions = {
  get?: string;
  post?: string;
  put?: string;
  delete?: string;
  custom?: string;
  headers?: Record<string, string>;
  auth?: boolean;
  cors?: string[];
};

/**
 * Decorates a method as an HTTP endpoint for this agent.
 */
export function endpoint(opts: EndpointDecoratorOptions) {
  return function (
    target: Object,
    propertyKey: string | symbol,
    descriptor: PropertyDescriptor,
  ) {
    const className = target.constructor.name;
    const methodName = String(propertyKey);

    const classMetadata = TypeMetadata.get(className);
    if (!classMetadata) {
      throw new Error(
        `Class metadata not found for agent ${className}. Ensure metadata is generated.`,
      );
    }

    const pathAndQuery = split_path_and_query(
      opts.get || opts.post || opts.put || opts.delete || opts.custom || '',
    );

    const path = pathAndQuery.path;
    const query = pathAndQuery.query;

    let httpMethod: HttpEndpointDetails['httpMethod'];

    if (opts.get) {
      httpMethod = { tag: 'get' };
    } else if (opts.post) {
      httpMethod = { tag: 'post' };
    } else if (opts.put) {
      httpMethod = { tag: 'put' };
    } else if (opts.delete) {
      httpMethod = { tag: 'delete' };
    } else if (opts.custom) {
      httpMethod = { tag: 'custom', val: opts.custom };
    } else {
      throw new Error(
        `Endpoint decorator must specify one of get/post/put/delete/custom for method ${methodName}`,
      );
    }

    if (!path.startsWith('/')) {
      throw new Error(
        `Endpoint path must start with '/'. Method: ${methodName}`,
      );
    }

    const pathSuffix: PathSegment[] = parsePath(path);

    const headerVars: HeaderVariable[] = Object.entries(opts.headers || {}).map(
      ([headerName, variableName]) => ({ headerName, variableName }),
    );

    const queryVars: QueryVariable[] = query ? parseQuery(query) || [] : [];
    const authDetails: AuthDetails = { required: opts.auth ?? true };
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
