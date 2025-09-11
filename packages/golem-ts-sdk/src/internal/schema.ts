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
import * as Option from '../newTypes/option';
import { AgentMethod, DataSchema, ElementSchema } from 'golem:agent/common';
import * as WitType from './mapping/types/WitType';
import { AgentClassName } from '../newTypes/agentClassName';
import { AgentMethodMetadataRegistry } from './registry/agentMethodMetadataRegistry';
import {
  ClassMetadata,
  ConstructorArg,
  MethodParams,
} from '@golemcloud/golem-ts-types-core';
import { TypeMappingScope } from './mapping/types/scope';

export function getConstructorDataSchema(
  agentClassName: AgentClassName,
  classType: ClassMetadata,
): Either.Either<DataSchema, string> {
  const constructorParamInfos: readonly ConstructorArg[] = classType.constructorArgs;

  const constructorParamTypes = Either.all(
    constructorParamInfos.map((paramInfo) =>
      WitType.fromTsType(
        paramInfo.type,
        Option.some(
          TypeMappingScope.constructor(
            agentClassName.value,
            paramInfo.name,
            paramInfo.type.optional,
          ),
        ),
      ),
    ),
  );

  const constructDataSchemaResult = Either.map(constructorParamTypes, (paramType) => {
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
  });

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
    return Either.left(`No metadata found for agent class ${agentClassName.value}`);
  }

  const methodMetadata = Array.from(classMetadata.methods.entries());

  return Either.all(
    methodMetadata.map((methodInfo) => {
      const methodName = methodInfo[0];
      const signature = methodInfo[1];

      const parameters: MethodParams = signature.methodParams;

      const returnType: Type.Type = signature.returnType;

      const baseMeta =
        AgentMethodMetadataRegistry.lookup(agentClassName)?.get(methodName) ?? {};

      const inputSchemaEither = buildMethodInputSchema(methodName, parameters);

      if (Either.isLeft(inputSchemaEither)) {
        return Either.left(inputSchemaEither.val);
      }

      const inputSchema = inputSchemaEither.val;

      const outputSchemaEither = buildOutputSchema(returnType);

      if (Either.isLeft(outputSchemaEither)) {
        return Either.left(
          `Failed to construct output schema for method ${methodName}: ${outputSchemaEither.val}.`,
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

export function buildMethodInputSchema(
  methodName: string,
  paramTypes: MethodParams,
): Either.Either<DataSchema, string> {
  const result = Either.all(
    Array.from(paramTypes).map((parameterInfo) =>
      Either.mapBoth(
        convertToElementSchema(
          parameterInfo[1],
          Option.some(
            TypeMappingScope.method(
              methodName,
              parameterInfo[0],
              parameterInfo[1].optional,
            ),
          ),
        ),
        (result) => {
          return [parameterInfo[0], result] as [string, ElementSchema];
        },
        (err) =>
          `Method: \`${methodName}\`, Parameter: \`${parameterInfo[0]}\`. Error: ${err}`,
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
  const undefinedSchema = handleUndefinedReturnType(returnType);

  if (Option.isSome(undefinedSchema)) {
    return Either.right(undefinedSchema.val);
  }

  return Either.map(convertToElementSchema(returnType, Option.none()), (result) => {
    return {
      tag: 'tuple',
      val: [['return-value', result]],
    };
  });
}

function convertToElementSchema(
  type: Type.Type,
  scope: Option.Option<TypeMappingScope>,
): Either.Either<ElementSchema, string> {
  return Either.map(WitType.fromTsType(type, scope), (witType) => {
    return {
      tag: 'component-model',
      val: witType,
    };
  });
}

function handleUndefinedReturnType(returnType: Type.Type): Option.Option<DataSchema> {
  switch (returnType.kind) {
    case 'null':
      return Option.some({
        tag: 'tuple',
        val: [],
      });

    case 'undefined':
      return Option.some({
        tag: 'tuple',
        val: [],
      });

    case 'void':
      return Option.some({
        tag: 'tuple',
        val: [],
      });

    case 'promise':
      const elementType = returnType.element;
      return handleUndefinedReturnType(elementType);

    default:
      return Option.none();
  }
}
