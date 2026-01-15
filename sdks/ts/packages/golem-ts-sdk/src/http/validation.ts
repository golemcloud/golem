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

import { AgentConstructor, HttpMountDetails } from 'golem:agent/common';

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

// Ensures that all agent constructor variables are provided
// by the HTTP mount, either via path variables or header variables.
export function validateHttpMountWithConstructor(
  agentMount: HttpMountDetails,
  agentConstructor: AgentConstructor,
) {
  const constructorVars = collectConstructorVars(agentConstructor);

  validateMountVariablesExistInConstructor(agentMount, constructorVars);
  validateConstructorVarsAreSatisfied(agentMount, constructorVars);
}

function collectConstructorVars(
  agentConstructor: AgentConstructor,
): Set<string> {
  return new Set(agentConstructor.inputSchema.val.map(([name]) => name));
}

function validateMountVariablesExistInConstructor(
  agentMount: HttpMountDetails,
  constructorVars: Set<string>,
) {
  for (const { headerName, variableName } of agentMount.headerVars) {
    if (!constructorVars.has(variableName)) {
      throw new Error(
        `HTTP mount header variable "${variableName}" (from header "${headerName}") ` +
          `is not defined in the agent constructor.`,
      );
    }
  }

  for (const [segmentIndex, segment] of agentMount.pathPrefix.entries()) {
    for (const node of segment.concat) {
      if (node.tag === 'path-variable') {
        const variableName = node.val.variableName;

        if (!constructorVars.has(variableName)) {
          throw new Error(
            `HTTP mount path variable "${variableName}" ` +
              `(in path segment ${segmentIndex}) ` +
              `is not defined in the agent constructor.`,
          );
        }
      }
    }
  }
}

function validateConstructorVarsAreSatisfied(
  agentMount: HttpMountDetails,
  constructorVars: Set<string>,
) {
  const providedVars = collectHttpMountVariables(agentMount);

  for (const constructorVar of constructorVars) {
    if (!providedVars.has(constructorVar)) {
      throw new Error(
        `Agent constructor variable "${constructorVar}" ` +
          `is not provided by the HTTP mount (path or headers).`,
      );
    }
  }
}

function collectHttpMountVariables(agentMount: HttpMountDetails): Set<string> {
  const vars = new Set<string>();

  for (const { variableName } of agentMount.headerVars) {
    vars.add(variableName);
  }

  for (const segment of agentMount.pathPrefix) {
    for (const node of segment.concat) {
      if (node.tag === 'path-variable') {
        vars.add(node.val.variableName);
      }
    }
  }

  return vars;
}
