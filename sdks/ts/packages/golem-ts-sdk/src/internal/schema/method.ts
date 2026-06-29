// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

import { Type } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../newTypes/either';
import { ReadOnlyConfig } from 'golem:agent/common@2.0.0';
import { AgentMethodRegistry } from '../registry/agentMethodRegistry';
import { ClassMetadata, MethodParams } from '@golemcloud/golem-ts-types-core';
import { validateMethodName } from './helpers';
import { resolveMethodInputParams } from './methodInput';
import { resolveMethodOutput } from './methodOutput';
import { RuntimeOutput, RuntimeParam } from '../typeInfoInternal';
import { EnrichedMethod } from './agentType';

export function resolveAgentMethods(
  classMetadata: ClassMetadata,
  agentClassName: string,
): EnrichedMethod[] {
  const baseError = `Schema generation failed for agent class ${agentClassName}`;

  if (!classMetadata) {
    throw new Error(`${baseError}. No metadata found for agent class ${agentClassName}`);
  }

  return Array.from(classMetadata.methods.entries()).map(([methodName, signature]) => {
    const { methodParams, returnType } = signature;

    validateMethodNameOrThrow(methodName, baseError);

    const params = resolveInputParamsOrThrow(agentClassName, methodName, methodParams, baseError);
    const output = resolveOutputOrThrow(methodName, returnType, baseError);

    AgentMethodRegistry.setReturnType(agentClassName, methodName, output);

    const baseMeta = AgentMethodRegistry.get(agentClassName)?.get(methodName) ?? {};

    const readOnly: ReadOnlyConfig | undefined =
      baseMeta.readOnly === undefined
        ? undefined
        : {
            cachePolicy: baseMeta.readOnly,
            usesPrincipal: params.some((p) => p.type.tag === 'principal'),
          };

    return {
      name: methodName,
      description: baseMeta.description ?? '',
      promptHint: baseMeta.prompt ?? '',
      httpEndpoint: baseMeta.httpEndpoint ?? [],
      readOnly,
      params,
      output,
    };
  });
}

function validateMethodNameOrThrow(methodName: string, baseError: string) {
  const validation = validateMethodName(methodName);
  if (Either.isLeft(validation)) {
    throw new Error(`${baseError}. ${validation.val}`);
  }
}

function resolveInputParamsOrThrow(
  agentClassName: string,
  methodName: string,
  parameters: MethodParams,
  baseError: string,
): RuntimeParam[] {
  const result = resolveMethodInputParams(agentClassName, methodName, parameters);
  if (Either.isLeft(result)) {
    throw new Error(`${baseError}. ${result.val}`);
  }
  return result.val;
}

function resolveOutputOrThrow(
  methodName: string,
  returnType: Type.Type,
  baseError: string,
): RuntimeOutput {
  const result = resolveMethodOutput(returnType);
  if (Either.isLeft(result)) {
    throw new Error(
      `${baseError}. Failed to construct output schema for method ${methodName} with return type ${returnType.name}: ${result.val}`,
    );
  }
  return result.val;
}
