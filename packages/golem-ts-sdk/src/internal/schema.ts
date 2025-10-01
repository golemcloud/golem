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
import { AgentMethodRegistry } from './registry/agentMethodRegistry';
import {
  ClassMetadata,
  ConstructorArg,
  MethodParams,
} from '@golemcloud/golem-ts-types-core';
import { TypeMappingScope } from './mapping/types/scope';
import { AgentConstructorParamRegistry } from './registry/agentConstructorParamRegistry';
import { AgentMethodParamRegistry } from './registry/agentMethodParamRegistry';
import { AgentConstructorRegistry } from './registry/agentConstructorRegistry';
import { AnalysedType, tuple } from './mapping/types/AnalysedType';

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
        AgentConstructorParamRegistry.setIfNotExists(
          agentClassName,
          paramInfo.name,
        );

        const elementSchema = getElementSchemaForUnstructuredText(paramType);

        if (Either.isLeft(elementSchema)) {
          return Either.left(
            `Failed to get element schema for unstructured-text parameter ${paramInfo.name}: ${elementSchema.val}`,
          );
        }

        const result: [string, ElementSchema] = [
          paramInfo.name,
          elementSchema.val,
        ];

        return Either.right(result);
      }

      if (paramTypeName && paramTypeName === 'UnstructuredBinary') {
        AgentConstructorParamRegistry.setIfNotExists(
          agentClassName,
          paramInfo.name,
        );

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

      return Either.map(witType, (typeInfo) => {
        const witType = typeInfo[0];
        const analysedType = typeInfo[1];

        AgentConstructorParamRegistry.setAnalysedType(
          agentClassName,
          paramInfo.name,
          analysedType,
        );

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

      const outputSchemaEither: Either.Either<
        [Option.Option<AnalysedType>, DataSchema],
        string
      > = buildOutputSchema(returnType);

      if (Either.isLeft(outputSchemaEither)) {
        return Either.left(
          `Failed to construct output schema for method ${methodName} with return type ${returnType.name}: ${outputSchemaEither.val}.`,
        );
      }

      const [analysedType, outputSchema] = outputSchemaEither.val;

      if (Option.isSome(analysedType)) {
        AgentMethodRegistry.setReturnType(agentClassName, methodName, {
          tag: 'analysed',
          val: analysedType.val,
        });
      } else {
        switch (outputSchema.tag) {
          case 'tuple':
            const value = outputSchema.val[0][1];

            switch (value.tag) {
              case 'component-model':
                break;
              case 'unstructured-text':
                AgentMethodRegistry.setReturnType(agentClassName, methodName, {
                  tag: 'unstructured-text',
                  val: value.val,
                });
                break;
              case 'unstructured-binary':
                AgentMethodRegistry.setReturnType(agentClassName, methodName, {
                  tag: 'unstructured-binary',
                  val: value.val,
                });
                break;
            }
            break;

          case 'multimodal':
            break;
        }
      }

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

  const agentClass = AgentMethodRegistry.lookup(agentClassName);

  const isMultiModal = agentClass?.get(methodName)?.multimodal ?? false;

  if (isMultiModal) {
    return Either.map(result, (res) => {
      return {
        tag: 'multimodal',
        val: res,
      };
    });
  }

  return Either.map(result, (res) => {
    return {
      tag: 'tuple',
      val: res,
    };
  });
}

export function buildOutputSchema(
  returnType: Type.Type,
): Either.Either<[Option.Option<AnalysedType>, DataSchema], string> {
  const undefinedSchema = handleUndefinedReturnType(returnType);

  if (Option.isSome(undefinedSchema)) {
    return Either.right([
      Option.some(tuple(undefined, 'undefined', [])),
      undefinedSchema.val,
    ]);
  }

  if (
    returnType.kind === 'promise' &&
    returnType.element.name === 'UnstructuredText'
  ) {
    const elementSchema = getElementSchemaForUnstructuredText(
      returnType.element,
    );

    if (Either.isLeft(elementSchema)) {
      return Either.left(
        `Failed to get element schema for unstructured-text return type: ${elementSchema.val}`,
      );
    }

    return Either.right([
      Option.none(),
      {
        tag: 'tuple',
        val: [['return-value', elementSchema.val]],
      },
    ]);
  }

  if (returnType.name === 'UnstructuredText') {
    const elementSchema = getElementSchemaForUnstructuredText(returnType);

    if (Either.isLeft(elementSchema)) {
      return Either.left(
        `Failed to get element schema for unstructured-text return type: ${elementSchema.val}`,
      );
    }

    return Either.right([
      Option.none(),
      {
        tag: 'tuple',
        val: [['return-value', elementSchema.val]],
      },
    ]);
  }

  const schema: Either.Either<
    [Option.Option<AnalysedType>, ElementSchema],
    string
  > = Either.map(WitType.fromTsType(returnType, Option.none()), (typeInfo) => {
    const witType = typeInfo[0];
    const analysedType = typeInfo[1];

    return [
      Option.some(analysedType),
      {
        tag: 'component-model',
        val: witType,
      },
    ];
  });

  return Either.map(schema, (result) => {
    return [
      result[0],
      {
        tag: 'tuple',
        val: [['return-value', result[1]]],
      },
    ];
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
    AgentMethodParamRegistry.ensureMeta(
      agentClassName,
      methodName,
      parameterName,
    );

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
    AgentMethodParamRegistry.ensureMeta(
      agentClassName,
      methodName,
      parameterName,
    );

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

  return Either.map(WitType.fromTsType(parameterType, scope), (typeInfo) => {
    const witType = typeInfo[0];
    const analysedType = typeInfo[1];

    AgentMethodParamRegistry.setAnalysedType(
      agentClassName,
      methodName,
      parameterName,
      analysedType,
    );

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

function getElementSchemaForUnstructuredText(
  paramType: Type.Type,
): Either.Either<ElementSchema, string> {
  const languageCodes = getLanguageCodes(paramType);

  if (Either.isLeft(languageCodes)) {
    return Either.left(`Failed to get language code: ${languageCodes.val}`);
  }

  const elementSchema: ElementSchema = languageCodes
    ? {
        tag: 'unstructured-text',
        val: {
          restrictions: languageCodes.val.map((code) => ({
            languageCode: code,
          })),
        },
      }
    : { tag: 'unstructured-text', val: {} };

  return Either.right(elementSchema);
}

export function getLanguageCodes(
  type: Type.Type,
): Either.Either<string[], string> {
  if (type.name === 'UnstructuredText' && type.kind === 'union') {
    const parameterTypes: Type.Type[] = type.typeParams ?? [];

    if (parameterTypes.length !== 1) {
      return Either.right([]);
    }

    const paramType: Type.Type = parameterTypes[0];

    if (paramType.kind === 'tuple') {
      const elem = paramType.elements;

      return Either.all(
        elem.map((v) => {
          if (v.kind === 'literal') {
            if (!v.literalValue) {
              return Either.left('language code literal has no value');
            }
            return Either.right(v.literalValue);
          } else {
            return Either.left('language code is not a literal');
          }
        }),
      );
    } else {
      return Either.left('unknown parameter type for UnstructuredText');
    }
  }

  return Either.left(
    `Type mismatch. Expected UnstructuredText, Found ${type.name}`,
  );
}
