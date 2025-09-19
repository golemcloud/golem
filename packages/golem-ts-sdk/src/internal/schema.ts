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
import {
  AgentMethod,
  DataSchema,
  ElementSchema,
  TextDescriptor,
  TextType,
} from 'golem:agent/common';
import * as WitType from './mapping/types/WitType';
import { AgentClassName } from '../newTypes/agentClassName';
import { AgentMethodRegistry } from './registry/agentMethodRegistry';
import {
  ClassMetadata,
  ConstructorArg,
  MethodParams,
} from '@golemcloud/golem-ts-types-core';
import { TypeMappingScope } from './mapping/types/scope';
import { languageCodes } from '../decorators';
import { AgentConstructorParamRegistry } from './registry/agentConstructorParamRegistry';
import { AgentMethodParamRegistry } from './registry/agentMethodParamRegistry';
import { AgentConstructorRegistry } from './registry/agentConstructorRegistry';

export function getConstructorDataSchema(
  agentClassName: AgentClassName,
  classType: ClassMetadata,
): Either.Either<DataSchema, string> {
  const constructorParamInfos: readonly ConstructorArg[] =
    classType.constructorArgs;

  const constructDataSchemaResult: Either.Either<
    [string, ElementSchema][],
    string
  > = Either.all(
    constructorParamInfos.map((paramInfo) => {
      const paramType = paramInfo.type;

      const paramTypeName = paramType.name;

      if (paramTypeName && paramTypeName === 'UnstructuredText') {
        const metadata = AgentConstructorParamRegistry.lookup(agentClassName);

        const languageCodes = metadata?.get(paramInfo.name)?.languageCodes;

        const elementSchema: ElementSchema = languageCodes
          ? {
              tag: 'unstructured-text',
              val: {
                restrictions: languageCodes.map((code) => ({
                  languageCode: code,
                })),
              },
            }
          : { tag: 'unstructured-text', val: {} };

        const result: [string, ElementSchema] = [paramInfo.name, elementSchema];

        return Either.right(result);
      }

      if (paramTypeName && paramTypeName === 'UnstructuredBinary') {
        const metadata = AgentConstructorParamRegistry.lookup(agentClassName);

        const mimeTypes = metadata?.get(paramInfo.name)?.mimeTypes;

        const elementSchema: ElementSchema = mimeTypes
          ? {
              tag: 'unstructured-binary',
              val: {
                restrictions: mimeTypes.map((mimeType) => ({
                  mimeType: mimeType,
                })),
              },
            }
          : { tag: 'unstructured-binary', val: {} };

        const result: [string, ElementSchema] = [paramInfo.name, elementSchema];

        return Either.right(result);
      }

      const witType = WitType.fromTsType(
        paramInfo.type,
        Option.some(
          TypeMappingScope.constructor(
            agentClassName.value,
            paramInfo.name,
            paramInfo.type.optional,
          ),
        ),
      );

      return Either.map(witType, (witType) => {
        const elementSchema: ElementSchema = {
          tag: 'component-model',
          val: witType,
        };
        return [paramInfo.name, elementSchema];
      });
    }),
  );

  const constructorParam = AgentConstructorRegistry.lookup(agentClassName);

  const isMultiModal = constructorParam?.multimodal ?? false;

  if (isMultiModal) {
    return Either.map(constructDataSchemaResult, (nameAndElementSchema) => {
      return {
        tag: 'multimodal',
        val: nameAndElementSchema,
      };
    });
  }

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
        AgentMethodRegistry.lookup(agentClassName)?.get(methodName) ?? {};

      const inputSchemaEither = buildMethodInputSchema(
        agentClassName,
        methodName,
        parameters,
      );

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
  agentClassName: AgentClassName,
  methodName: string,
  paramTypes: MethodParams,
): Either.Either<DataSchema, string> {
  const result = Either.all(
    Array.from(paramTypes).map((parameterInfo) =>
      Either.mapBoth(
        convertToElementSchema(
          agentClassName,
          methodName,
          parameterInfo[0],
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

  const schema: Either.Either<ElementSchema, string> = Either.map(
    WitType.fromTsType(returnType, Option.none()),
    (witType) => {
      return {
        tag: 'component-model',
        val: witType,
      };
    },
  );

  return Either.map(schema, (result) => {
    return {
      tag: 'tuple',
      val: [['return-value', result]],
    };
  });
}

function convertToElementSchema(
  agentClassName: AgentClassName,
  methodName: string,
  parameterName: string,
  parameterType: Type.Type,
  scope: Option.Option<TypeMappingScope>,
): Either.Either<ElementSchema, string> {
  const paramTypeName = parameterType.name;

  if (paramTypeName && paramTypeName === 'UnstructuredText') {
    const methodMetadata = AgentMethodParamRegistry.lookup(agentClassName);

    const parameterMetadata = methodMetadata?.get(methodName);

    const languageCodes = parameterMetadata?.get(parameterName)?.languageCode;

    const elementSchema: ElementSchema = languageCodes
      ? {
          tag: 'unstructured-text',
          val: {
            restrictions: languageCodes.map((code) => ({
              languageCode: code,
            })),
          },
        }
      : { tag: 'unstructured-text', val: {} };

    return Either.right(elementSchema);
  }

  if (paramTypeName && paramTypeName === 'UnstructuredBinary') {
    const methodMetadata = AgentMethodParamRegistry.lookup(agentClassName);

    const parameterMetadata = methodMetadata?.get(methodName);

    const mimeTypes = parameterMetadata?.get(parameterName)?.mimeTypes;

    const elementSchema: ElementSchema = mimeTypes
      ? {
          tag: 'unstructured-binary',
          val: {
            restrictions: mimeTypes.map((mimeType) => ({
              mimeType: mimeType,
            })),
          },
        }
      : { tag: 'unstructured-binary', val: {} };

    return Either.right(elementSchema);
  }

  return Either.map(WitType.fromTsType(parameterType, scope), (witType) => {
    return {
      tag: 'component-model',
      val: witType,
    };
  });
}

function handleUndefinedReturnType(
  returnType: Type.Type,
): Option.Option<DataSchema> {
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
