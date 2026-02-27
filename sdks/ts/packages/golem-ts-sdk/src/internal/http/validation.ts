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
  DataSchema,
  HttpEndpointDetails,
  HttpMountDetails,
} from 'golem:agent/common@1.5.0';
import { AgentMethodParamRegistry } from '../registry/agentMethodParamRegistry';
import { AgentConstructorParamRegistry } from '../registry/agentConstructorParamRegistry';
import { TypeInfoInternal } from '../typeInfoInternal';

export function validateHttpMount(
  agentClassName: string,
  agentMount: HttpMountDetails,
  agentConstructor: AgentConstructor,
) {
  const parametersForPrincipal =
    AgentConstructorParamRegistry.getParametersForPrincipal(agentClassName);

  const constructorInputParams = collectConstructorInputParameterNames(agentConstructor);

  validateNoCatchAllInHttpMount(agentClassName, agentMount);
  validateConstructorParamsAreHttpSafe(agentClassName, agentConstructor);
  validateMountVariablesAreNotPrincipal(agentMount, parametersForPrincipal);
  validateMountVariablesExistInConstructor(agentMount, constructorInputParams);
  validateConstructorVarsAreSatisfied(agentMount, constructorInputParams);
}

export function validateHttpEndpoint(
  agentClassName: string,
  agentMethod: AgentMethod,
  httpMountDetails: HttpMountDetails | undefined,
) {
  if (agentMethod.httpEndpoint.length === 0) {
    return;
  }

  validateMountIsDefinedForHttpEndpoint(agentClassName, agentMethod, httpMountDetails);

  const parameterTypes = AgentMethodParamRegistry.getParametersAndType(
    agentClassName,
    agentMethod.name,
  );

  const methodVarsWithoutAutoInjectedVariables = collectMethodInputVars(agentMethod.inputSchema);

  for (const endpoint of agentMethod.httpEndpoint) {
    validateEndpointVariables(endpoint, methodVarsWithoutAutoInjectedVariables, parameterTypes);
  }
}

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

function collectMethodInputVars(schema: DataSchema): Set<string> {
  return new Set(schema.val.map(([name]) => name));
}

function validateMountIsDefinedForHttpEndpoint(
  agentClassName: string,
  agentMethod: AgentMethod,
  httpMountDetails: HttpMountDetails | undefined,
) {
  if (!httpMountDetails && agentMethod.httpEndpoint.length > 0) {
    throw new Error(
      `Agent method '${agentMethod.name}' of '${agentClassName}' defines HTTP endpoints ` +
        `but the agent is not mounted over HTTP. Please specify mount details in 'agent' decorator.`,
    );
  }
}

function validateNoCatchAllInHttpMount(agentClassName: string, agentMount: HttpMountDetails) {
  const catchAllSegment = agentMount.pathPrefix.find(
    (segment) => segment.tag === 'remaining-path-variable',
  );

  if (catchAllSegment) {
    throw new Error(
      `HTTP mount for agent '${agentClassName}' cannot contain catch-all path variable '${catchAllSegment.val.variableName}'`,
    );
  }
}

function validateEndpointVariables(
  endpoint: HttpEndpointDetails,
  methodVars: Set<string>,
  parameterTypes: Map<string, TypeInfoInternal>,
) {
  const principalParams = getPrincipalParams(parameterTypes);
  const unstructuredBinaryParams = getUnstructuredBinaryParams(parameterTypes);

  function validateVariable(
    variableName: string,
    location: 'header' | 'query' | 'path',
    binaryError: string,
  ) {
    if (principalParams.has(variableName)) {
      throw new Error(
        `HTTP endpoint ${location} variable '${variableName}' cannot be used for parameters of type 'Principal'`,
      );
    }

    if (unstructuredBinaryParams.has(variableName)) {
      throw new Error(binaryError);
    }

    if (!methodVars.has(variableName)) {
      throw new Error(
        `HTTP endpoint ${location} variable '${variableName}' is not defined in method input parameters.`,
      );
    }
  }

  for (const { variableName } of endpoint.headerVars) {
    validateVariable(
      variableName,
      'header',
      `HTTP endpoint header variable '${variableName}' cannot be used for method parameters of type 'UnstructuredBinary'`,
    );
  }

  for (const { variableName } of endpoint.queryVars) {
    validateVariable(
      variableName,
      'query',
      `HTTP endpoint query variable '${variableName}' cannot be used when the method has a single 'UnstructuredBinary' parameter.`,
    );
  }

  for (const segment of endpoint.pathSuffix) {
    if (segment.tag === 'remaining-path-variable' || segment.tag === 'path-variable') {
      const name = segment.val.variableName;

      validateVariable(
        name,
        'path',
        `HTTP endpoint path variable "${name}" cannot be used when the method has a single 'UnstructuredBinary' parameter.`,
      );
    }
  }
}

function getPrincipalParams(parameterTypes: Map<string, TypeInfoInternal>): Set<string> {
  const methodVarsOfPrincipal = new Set<string>();

  for (const [varName, typeInfo] of parameterTypes.entries()) {
    if (typeInfo.tag === 'principal') {
      methodVarsOfPrincipal.add(varName);
    }
  }

  return methodVarsOfPrincipal;
}

function getUnstructuredBinaryParams(parameterTypes: Map<string, TypeInfoInternal>): Set<string> {
  const methodVarsOfPrincipal = new Set<string>();

  for (const [varName, typeInfo] of parameterTypes.entries()) {
    if (typeInfo.tag === 'unstructured-binary') {
      methodVarsOfPrincipal.add(varName);
    }
  }

  return methodVarsOfPrincipal;
}

function collectConstructorInputParameterNames(agentConstructor: AgentConstructor): Set<string> {
  return new Set(agentConstructor.inputSchema.val.map(([name]) => name));
}

function validateConstructorParamsAreHttpSafe(
  agentClassName: string,
  agentConstructor: AgentConstructor,
) {
  for (const [paramName, paramSchema] of agentConstructor.inputSchema.val) {
    if (paramSchema.tag === 'unstructured-binary') {
      throw new Error(
        `HTTP mount path variable '${paramName}' cannot be used for constructor parameters of type 'UnstructuredBinary'`,
      );
    }
  }
}

function validateMountVariablesAreNotPrincipal(
  agentMount: HttpMountDetails,
  parametersForPrincipal: Set<string>,
) {
  for (const segment of agentMount.pathPrefix) {
    if (segment.tag === 'path-variable') {
      const variableName = segment.val.variableName;
      if (parametersForPrincipal.has(variableName)) {
        throw new Error(
          `HTTP mount path variable '${variableName}' cannot be used for constructor parameters of type 'Principal'`,
        );
      }
    }
  }
}

function validateMountVariablesExistInConstructor(
  agentMount: HttpMountDetails,
  constructorVars: Set<string>,
) {
  for (const [segmentIndex, segment] of agentMount.pathPrefix.entries()) {
    if (segment.tag === 'path-variable') {
      const variableName = segment.val.variableName;

      if (!constructorVars.has(variableName)) {
        throw new Error(
          `HTTP mount path variable '${variableName}' ` +
            `(in path segment ${segmentIndex}) ` +
            `is not defined in the agent constructor.`,
        );
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
        `Agent constructor variable '${constructorVar}' ` +
          `is not provided by the HTTP mount path.`,
      );
    }
  }
}

function collectHttpMountVariables(agentMount: HttpMountDetails): Set<string> {
  const vars = new Set<string>();

  for (const segment of agentMount.pathPrefix) {
    if (segment.tag === 'path-variable') {
      vars.add(segment.val.variableName);
    }
  }

  return vars;
}
