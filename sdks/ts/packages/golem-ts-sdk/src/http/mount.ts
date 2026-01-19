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
  HeaderVariable,
  HttpMountDetails,
  PathSegment,
} from 'golem:agent/common';
import { parsePath } from './path';
import { rejectEmptyString, rejectQueryParamsInPath } from './validation';
import { AgentDecoratorOptions } from '../decorators/agent';

export type HeaderVariables = Record<string, string>;

export function getHttpMountDetails(
  agentDecoratorOptions?: AgentDecoratorOptions,
): HttpMountDetails | undefined {
  if (!agentDecoratorOptions?.mount!) return undefined;

  rejectQueryParamsInPath(agentDecoratorOptions.mount, 'mount');
  rejectEmptyString(agentDecoratorOptions.mount, 'mount');

  const pathPrefix = parsePath(agentDecoratorOptions.mount);
  const headerVars = parseHeaderVars(agentDecoratorOptions.headers);

  return {
    pathPrefix,
    queryVars: [],
    headerVars,
    authDetails: agentDecoratorOptions.auth
      ? { required: true }
      : { required: false },
    phantomAgent: false,
    corsOptions: {
      allowedPatterns: agentDecoratorOptions.cors ?? [],
    },
    webhookSuffix: parseWebhook(agentDecoratorOptions.webhookSuffix),
  };
}

function parseWebhook(webhook?: string): PathSegment[] {
  if (!webhook) return [];

  rejectQueryParamsInPath(webhook, 'webhook suffix');

  rejectEmptyString(webhook, 'webhook suffix');

  return parsePath(webhook);
}

function parseHeaderVars(headers?: HeaderVariables): HeaderVariable[] {
  if (!headers) return [];

  return Object.entries(headers).map(([headerName, variableName]) => {
    rejectEmptyString(variableName, 'header value name');
    rejectEmptyString(headerName, 'header variable name');

    return {
      headerName,
      variableName,
    };
  });
}
