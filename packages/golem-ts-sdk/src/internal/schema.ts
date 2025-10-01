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

        return Either.right([paramInfo.name, elementSchema.val]);
      }

      if (paramTypeName && paramTypeName === 'UnstructuredBinary') {
        AgentConstructorParamRegistry.setIfNotExists(
          agentClassName,
          paramInfo.name,
        );

        const elementSchema = getElementSchemaForUnstructuredBinary(paramType);

        if (Either.isLeft(elementSchema)) {
          return Either.left(
            `Failed to get element schema for unstructured-binary parameter ${paramInfo.name}: ${elementSchema.val}`,
          );
        }

        return Either.right([paramInfo.name, elementSchema.val]);
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

      // analysed-type exists for all types except unstructured types and binary
      if (Option.isSome(analysedType)) {
        AgentMethodRegistry.setReturnType(agentClassName, methodName, {
          tag: 'analysed',
          val: analysedType.val,
        });
      } else {
        // Special handling for unstructured types to set metadata in the param registry
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

  const unstructuredText = handleUnstructuredCase(
    returnType,
    'UnstructuredText',
    getElementSchemaForUnstructuredText,
  );
  if (Either.isRight(unstructuredText)) {
    return unstructuredText;
  }

  const unstructuredBinary = handleUnstructuredCase(
    returnType,
    'UnstructuredBinary',
    getElementSchemaForUnstructuredBinary,
  );
  if (Either.isRight(unstructuredBinary)) {
    return unstructuredBinary;
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

  return Either.map(schema, (result) => [
    result[0],
    { tag: 'tuple', val: [['return-value', result[1]]] },
  ]);
}

function handleUnstructuredCase(
  returnType: Type.Type,
  typeName: string,
  getElementSchema: (t: Type.Type) => Either.Either<ElementSchema, string>,
): Either.Either<[Option.Option<AnalysedType>, DataSchema], string> {
  const target =
    returnType.kind === 'promise' && returnType.element.name === typeName
      ? returnType.element
      : returnType.name === typeName
        ? returnType
        : null;

  if (!target) {
    return Either.left('not-special-case');
  }

  const elementSchema = getElementSchema(target);
  if (Either.isLeft(elementSchema)) {
    return Either.left(
      `Failed to get element schema for ${typeName} return type: ${elementSchema.val}`,
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

    return getElementSchemaForUnstructuredText(parameterType);
  }

  if (paramTypeName && paramTypeName === 'UnstructuredBinary') {
    AgentMethodParamRegistry.ensureMeta(
      agentClassName,
      methodName,
      parameterName,
    );

    return getElementSchemaForUnstructuredBinary(parameterType);
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

function getElementSchemaForUnstructuredBinary(
  paramType: Type.Type,
): Either.Either<ElementSchema, string> {
  const mimeTypes = getMimeTypes(paramType);

  if (Either.isLeft(mimeTypes)) {
    return Either.left(`Failed to get mime types: ${mimeTypes.val}`);
  }

  const elementSchema: ElementSchema =
    mimeTypes.val.length > 0
      ? {
          tag: 'unstructured-binary',
          val: {
            restrictions: mimeTypes.val.map((type) => ({
              mimeType: type,
            })),
          },
        }
      : { tag: 'unstructured-binary', val: {} };

  return Either.right(elementSchema);
}

function getElementSchemaForUnstructuredText(
  paramType: Type.Type,
): Either.Either<ElementSchema, string> {
  const languageCodes = getLanguageCodes(paramType);

  if (Either.isLeft(languageCodes)) {
    return Either.left(`Failed to get language code: ${languageCodes.val}`);
  }

  const elementSchema: ElementSchema =
    languageCodes.val.length > 0
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

export function getMimeTypes(type: Type.Type): Either.Either<string[], string> {
  if (type.name === 'UnstructuredBinary' && type.kind === 'union') {
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
              return Either.left('mime type literal has no value');
            }
            return Either.right(v.literalValue);
          } else {
            return Either.left('mime type is not a literal');
          }
        }),
      );
    } else {
      return Either.left('unknown parameter type for UnstructuredBinary');
    }
  }

  return Either.left(
    `Type mismatch. Expected UnstructuredBinary, Found ${type.name}`,
  );
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
