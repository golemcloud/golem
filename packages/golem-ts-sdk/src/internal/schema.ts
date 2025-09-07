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
import * as Either from '../newTypes/either';
import { AgentMethod, DataSchema, ElementSchema } from 'golem:agent/common';
import * as WitType from './mapping/types/WitType';
import { AgentClassName } from '../newTypes/agentClassName';
import { AgentMethodMetadataRegistry } from './registry/agentMethodMetadataRegistry';
import {
  ClassMetadata,
  ConstructorArg,
  MethodParams,
} from '@golemcloud/golem-ts-types-core';

export function getConstructorDataSchema(
  classType: ClassMetadata,
): Either.Either<DataSchema, string> {
  const constructorParamInfos: readonly ConstructorArg[] =
    classType.constructorArgs;

  const constructorParamTypes = Either.all(
    constructorParamInfos.map((paramInfo) =>
      WitType.fromTsType(paramInfo.type),
    ),
  );

  const constructDataSchemaResult = Either.map(
    constructorParamTypes,
    (paramType) => {
      return paramType.map((paramType, idx) => {
        const paramName = constructorParamInfos[idx].name;
        return [
          paramName,
          {
            tag: 'component-model',
            val: paramType,
          },
        ] as [string, ElementSchema];
      });
    },
  );

  return Either.map(constructDataSchemaResult, (nameAndElementSchema) => {
    return {
      tag: 'tuple',
      val: nameAndElementSchema,
    };
  });
}

export function getAgentMethodSchema(
  classMetadata: ClassMetadata,
  agentClassName: AgentClassName,
): Either.Either<AgentMethod[], string> {
  if (!classMetadata) {
    return Either.left(
      `No metadata found for agent class ${agentClassName.value}`,
    );
  }

  const methodMetadata = Array.from(classMetadata.methods.entries());

  return Either.all(
    methodMetadata.map((methodInfo) => {
      const methodName = methodInfo[0];
      const signature = methodInfo[1];

      const parameters: MethodParams = signature.methodParams;

      const returnType: Type.Type = signature.returnType;

      const baseMeta =
        AgentMethodMetadataRegistry.lookup(agentClassName)?.get(methodName) ??
        {};

      const inputSchemaEither = buildInputSchema(methodName, parameters);

      if (Either.isLeft(inputSchemaEither)) {
        return Either.left(inputSchemaEither.val);
      }

      const inputSchema = inputSchemaEither.val;

      const outputSchemaEither = buildOutputSchema(returnType);

      if (Either.isLeft(outputSchemaEither)) {
        return Either.left(
          `Failed to construct output schema for method ${methodName}: ${outputSchemaEither.val}`,
        );
      }

      const outputSchema = outputSchemaEither.val;

      return Either.right({
        name: methodName,
        description: baseMeta.description ?? '',
        promptHint: baseMeta.prompt ?? '',
        inputSchema: inputSchema,
        outputSchema: outputSchema,
      });
    }),
  );
}

export function buildInputSchema(
  methodName: string,
  paramTypes: MethodParams,
): Either.Either<DataSchema, string> {
  const result = Either.all(
    Array.from(paramTypes).map((parameterInfo) =>
      Either.mapBoth(
        convertToElementSchema(parameterInfo[1]),
        (result) => {
          return [parameterInfo[0], result] as [string, ElementSchema];
        },
        (err) =>
          `${err}, found in method \`${methodName}\`, parameter \`${parameterInfo[0]}\`. Please replace this parameter type with a simpler, supported type"`,
      ),
    ),
  );

  return Either.map(result, (res) => {
    return {
      tag: 'tuple',
      val: res,
    };
  });
}

export function buildOutputSchema(
  returnType: Type.Type,
): Either.Either<DataSchema, string> {
  return Either.map(convertToElementSchema(returnType), (result) => {
    return {
      tag: 'tuple',
      val: [['return-value', result]],
    };
  });
}

function convertToElementSchema(
  type: Type.Type,
): Either.Either<ElementSchema, string> {
  return Either.map(WitType.fromTsType(type), (witType) => {
    return {
      tag: 'component-model',
      val: witType,
    };
  });
}
