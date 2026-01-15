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
  AgentConstructor,
  AgentMethod,
  HttpMountDetails,
} from 'golem:agent/common';

export function rejectEmptyString(name: string, entityName: string) {
  if (name.length === 0) {
    throw new Error(`HTTP ${entityName} must not be empty`);
  }
}

export function rejectQueryParamsInPath(path: string, entityName: string) {
  if (path.includes('?')) {
    throw new Error(`HTTP ${entityName} must not contain query parameters`);
  }
}

// Ensures that all variables referenced by the HTTP mount
// (path and headers) are declared in the agent constructor.
export function validateHttpMountWithConstructor(
  agentMount: HttpMountDetails,
  agentConstructor: AgentConstructor,
) {
  const constructorVars = new Set(
    agentConstructor.inputSchema.val.map(([name]) => name),
  );

  validateHeaderVarsAgainstConstructor(agentMount, constructorVars);
  validatePathVarsAgainstConstructor(agentMount, constructorVars);
}

function validateHeaderVarsAgainstConstructor(
  agentMount: HttpMountDetails,
  constructorVars: Set<string>,
) {
  for (const { variableName } of agentMount.headerVars) {
    if (!constructorVars.has(variableName)) {
      throw new Error(
        `HTTP mount header variable "${variableName}" is not defined in the agent constructor variables.`,
      );
    }
  }
}

function validatePathVarsAgainstConstructor(
  agentMount: HttpMountDetails,
  constructorVars: Set<string>,
) {
  const pathVars = agentMount.pathPrefix
    .flatMap((segment) => segment.concat)
    .filter((node) => node.tag === 'path-variable')
    .map((node) => node.val.variableName);

  for (const variableName of pathVars) {
    if (!constructorVars.has(variableName)) {
      throw new Error(
        `HTTP mount path variable "${variableName}" is not defined in the agent constructor variables.`,
      );
    }
  }
}
