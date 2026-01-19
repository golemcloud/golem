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
import { AgentMethod, HttpMountDetails } from 'golem:agent/common';
import { AgentMethodRegistry } from './../registry/agentMethodRegistry';
import { ClassMetadata, MethodParams } from '@golemcloud/golem-ts-types-core';
import {
  getReturnTypeDataSchemaFromTypeInternal,
  TypeInfoInternal,
} from '../typeInfoInternal';
import { validateHttpEndpoint } from '../http/validation';
import { validateMethodName } from './helpers';
import { buildMethodInputSchema } from './methodInput';
import { buildOutputSchema } from './methodOutput';

export function getAgentMethodSchema(
  classMetadata: ClassMetadata,
  agentClassName: string,
  httpMountDetails: HttpMountDetails | undefined,
): AgentMethod[] {
  const baseError = `Schema generation failed for agent class ${agentClassName}`;

  if (!classMetadata) {
    throw new Error(
      `${baseError}. No metadata found for agent class ${agentClassName}`,
    );
  }

  const methodMetadata = Array.from(classMetadata.methods.entries());
  return methodMetadata.map((methodInfo) => {
    const methodName = methodInfo[0];
    const signature = methodInfo[1];
    const parameters: MethodParams = signature.methodParams;
    const returnType: Type.Type = signature.returnType;

    const methodNameValidation = validateMethodName(methodName);
    if (Either.isLeft(methodNameValidation)) {
      throw new Error(`${baseError}. ${methodNameValidation.val}`);
    }

    const baseMeta =
      AgentMethodRegistry.get(agentClassName)?.get(methodName) ?? {};

    const inputSchemaEither = buildMethodInputSchema(
      agentClassName,
      methodName,
      parameters,
    );

    if (Either.isLeft(inputSchemaEither)) {
      throw new Error(`${baseError}. ${inputSchemaEither.val}`);
    }

    const inputSchema = inputSchemaEither.val;

    const outputTypeInfoEither: Either.Either<TypeInfoInternal, string> =
      buildOutputSchema(returnType);

    if (Either.isLeft(outputTypeInfoEither)) {
      throw new Error(
        `${baseError}. Failed to construct output schema for method ${methodName} with return type ${returnType.name}: ${outputTypeInfoEither.val}.`,
      );
    }

    const outputTypeInfoInternal = outputTypeInfoEither.val;

    AgentMethodRegistry.setReturnType(
      agentClassName,
      methodName,
      outputTypeInfoInternal,
    );

    const outputSchemaEither = getReturnTypeDataSchemaFromTypeInternal(
      outputTypeInfoInternal,
    );

    if (Either.isLeft(outputSchemaEither)) {
      throw new Error(
        `${baseError}. Failed to get output data schema for method ${methodName}: ${outputSchemaEither.val}`,
      );
    }

    const outputSchema = outputSchemaEither.val;

    const agentMethod: AgentMethod = {
      name: methodName,
      description: baseMeta.description ?? '',
      promptHint: baseMeta.prompt ?? '',
      inputSchema: inputSchema,
      outputSchema: outputSchema,
      httpEndpoint: baseMeta.httpEndpoint ?? [],
    };

    // validateHttpEndpoint surely runs as part of building the agent
    validateHttpEndpoint(agentClassName, agentMethod, httpMountDetails);

    return {
      name: methodName,
      description: baseMeta.description ?? '',
      promptHint: baseMeta.prompt ?? '',
      inputSchema: inputSchema,
      outputSchema: outputSchema,
      httpEndpoint: baseMeta.httpEndpoint ?? [],
    };
  });
}
