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

import { Type } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../newTypes/either';
import { AgentMethod, DataSchema, HttpMountDetails } from 'golem:agent/common@1.5.0';
import { AgentMethodRegistry } from '../registry/agentMethodRegistry';
import { ClassMetadata, MethodParams } from '@golemcloud/golem-ts-types-core';
import { validateHttpEndpoint } from '../http/validation';
import { validateMethodName } from './helpers';
import { resolveMethodInputSchema } from './methodInput';
import { resolveMethodReturnDataSchema } from './methodOutput';

export function getAgentMethodSchema(
  classMetadata: ClassMetadata,
  agentClassName: string,
  httpMountDetails: HttpMountDetails | undefined,
): AgentMethod[] {
  const baseError = `Schema generation failed for agent class ${agentClassName}`;

  if (!classMetadata) {
    throw new Error(`${baseError}. No metadata found for agent class ${agentClassName}`);
  }

  return Array.from(classMetadata.methods.entries()).map(([methodName, signature]) => {
    const { methodParams, returnType } = signature;

    validateMethodNameOrThrow(methodName, baseError);

    const baseMeta = AgentMethodRegistry.get(agentClassName)?.get(methodName) ?? {};

    const inputSchema: DataSchema = resolveInputSchemaOrThrow(
      agentClassName,
      methodName,
      methodParams,
      baseError,
    );

    const outputSchema = resolveReturnSchemaOrThrow(
      agentClassName,
      methodName,
      returnType,
      baseError,
    );

    const agentMethod: AgentMethod = {
      name: methodName,
      description: baseMeta.description ?? '',
      promptHint: baseMeta.prompt ?? '',
      inputSchema,
      outputSchema,
      httpEndpoint: baseMeta.httpEndpoint ?? [],
    };

    validateHttpEndpoint(agentClassName, agentMethod, httpMountDetails);

    return agentMethod;
  });
}

function validateMethodNameOrThrow(methodName: string, baseError: string) {
  const validation = validateMethodName(methodName);

  if (Either.isLeft(validation)) {
    throw new Error(`${baseError}. ${validation.val}`);
  }
}

function resolveInputSchemaOrThrow(
  agentClassName: string,
  methodName: string,
  parameters: MethodParams,
  baseError: string,
): DataSchema {
  const inputSchemaEither = resolveMethodInputSchema(agentClassName, methodName, parameters);

  if (Either.isLeft(inputSchemaEither)) {
    throw new Error(`${baseError}. ${inputSchemaEither.val}`);
  }

  return inputSchemaEither.val;
}

function resolveReturnSchemaOrThrow(
  agentClassName: string,
  methodName: string,
  returnType: Type.Type,
  baseError: string,
): DataSchema {
  const returnSchemaEither = resolveMethodReturnDataSchema(agentClassName, methodName, returnType);

  if (Either.isLeft(returnSchemaEither)) {
    throw new Error(
      `${baseError}. Failed to construct output schema for method ${methodName} with return type ${returnType.name}: ${returnSchemaEither.val}`,
    );
  }

  return returnSchemaEither.val;
}
