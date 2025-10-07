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
  BinaryDescriptor,
  DataSchema,
  ElementSchema,
  TextDescriptor,
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
import { AgentConstructorParamRegistry } from './registry/agentConstructorParamRegistry';
import { AgentMethodParamRegistry } from './registry/agentMethodParamRegistry';
import { AnalysedType, tuple } from './mapping/types/AnalysedType';
import { TypeInfoInternal } from './registry/typeInfoInternal';

export function getConstructorDataSchema(
  agentClassName: AgentClassName,
  classType: ClassMetadata,
): Either.Either<DataSchema, string> {
  const constructorParamInfos: readonly ConstructorArg[] =
    classType.constructorArgs;

  if (
    constructorParamInfos.length === 1 &&
    constructorParamInfos[0].type.name === 'Multimodal'
  ) {
    const paramType = constructorParamInfos[0].type;

    if (paramType.name === 'Multimodal' && paramType.kind === 'array') {
      const elementType = paramType.element;
      const multimodalTypes: Type.Type[] =
        elementType.kind === 'union' ? elementType.unionTypes : [elementType];

      const multiModalDetails = getMultimodalDetails(
        agentClassName,
        multimodalTypes,
      );

      if (Either.isLeft(multiModalDetails)) {
        return Either.left(
          `Failed to get multimodal details: ${multiModalDetails.val}`,
        );
      }

      const typeInfoInternals = multiModalDetails.val.map(
        ([name, , typeInfo]) => [name, typeInfo] as [string, TypeInfoInternal],
      );

      const schemaDetails = multiModalDetails.val.map(
        ([name, schema]) => [name, schema] as [string, ElementSchema],
      );

      AgentConstructorParamRegistry.setType(
        agentClassName,
        constructorParamInfos[0].name,
        {
          tag: 'multimodal',
          tsType: paramType,
          types: typeInfoInternals,
        },
      );

      return Either.right({
        tag: 'multimodal',
        val: schemaDetails,
      });
    }
  }

  // For other type other than multimodal
  const constructDataSchemaResult: Either.Either<
    [string, ElementSchema][],
    string
  > = getParameterNameAndElementSchema(agentClassName, constructorParamInfos);

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
        AgentMethodRegistry.get(agentClassName)?.get(methodName) ?? {};

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
        [TypeDetails, DataSchema],
        string
      > = buildOutputSchema(agentClassName, returnType);

      if (Either.isLeft(outputSchemaEither)) {
        return Either.left(
          `Failed to construct output schema for method ${methodName} with return type ${returnType.name}: ${outputSchemaEither.val}.`,
        );
      }

      const [typeDetails, outputSchema] = outputSchemaEither.val;

      switch (typeDetails.tag) {
        case 'unstructured-text':
          switch (outputSchema.tag) {
            case 'tuple':
              const value = outputSchema.val[0][1];

              if (value.tag !== 'unstructured-text') {
                return Either.left(
                  `Output schema type mismatch for method ${methodName}. Expected unstructured-text, found ${value.tag}`,
                );
              }

              AgentMethodRegistry.setReturnType(agentClassName, methodName, {
                tag: 'unstructured-text',
                val: value.val,
                tsType: returnType,
              });

              break;

            default:
              return Either.left(
                `Output schema type mismatch for method ${methodName}. Expected tuple for unstructured-text, found ${outputSchema.tag}`,
              );
          }

          break;
        case 'analysed':
          AgentMethodRegistry.setReturnType(agentClassName, methodName, {
            tag: 'analysed',
            val: typeDetails.val,
            tsType: returnType,
          });
          break;
        case 'multimodal':
          AgentMethodRegistry.setReturnType(agentClassName, methodName, {
            tag: 'multimodal',
            tsType: returnType,
            types: typeDetails.val,
          });
          break;
        case 'unstructured-binary':
          switch (outputSchema.tag) {
            case 'tuple':
              const value = outputSchema.val[0][1];

              if (value.tag !== 'unstructured-binary') {
                return Either.left(
                  `Output schema type mismatch for method ${methodName}. Expected unstructured-binary, found ${value.tag}`,
                );
              }

              AgentMethodRegistry.setReturnType(agentClassName, methodName, {
                tag: 'unstructured-binary',
                val: value.val,
                tsType: returnType,
              });

              break;

            default:
              return Either.left(
                `Output schema type mismatch for method ${methodName}. Expected tuple for unstructured-binary, found ${outputSchema.tag}`,
              );
          }

          break;
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
  const paramTypesArray = Array.from(paramTypes);

  if (
    paramTypesArray.length === 1 &&
    paramTypesArray[0][1].name === 'Multimodal'
  ) {
    const paramType = paramTypesArray[0][1];

    if (paramType.name === 'Multimodal' && paramType.kind === 'array') {
      const elementType = paramType.element;

      const multimodalTypes =
        elementType.kind === 'union' ? elementType.unionTypes : [elementType];

      const multiModalDetails = getMultimodalDetails(
        agentClassName,
        multimodalTypes,
      );

      if (Either.isLeft(multiModalDetails)) {
        return Either.left(
          `Failed to get multimodal details: ${multiModalDetails.val}`,
        );
      }

      const typeInfoInternals = multiModalDetails.val.map(
        ([name, , typeInfo]) => [name, typeInfo] as [string, TypeInfoInternal],
      );

      const schemaDetails = multiModalDetails.val.map(
        ([name, schema]) => [name, schema] as [string, ElementSchema],
      );

      AgentMethodParamRegistry.setType(
        agentClassName,
        methodName,
        paramTypesArray[0][0],
        {
          tag: 'multimodal',
          tsType: paramType,
          types: typeInfoInternals,
        },
      );

      return Either.right({
        tag: 'multimodal',
        val: schemaDetails,
      });
    } else {
      return Either.left('Multimodal type is not an array');
    }
  }

  const result = Either.all(
    paramTypesArray.map((parameterInfo) =>
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

  const agentClass = AgentMethodRegistry.get(agentClassName);

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

type TypeDetails =
  | { tag: 'analysed'; val: AnalysedType }
  | { tag: 'multimodal'; val: [string, TypeInfoInternal][] }
  | { tag: 'unstructured-text' }
  | { tag: 'unstructured-binary' };

export function buildOutputSchema(
  agentClassName: AgentClassName,
  returnType: Type.Type,
): Either.Either<[TypeDetails, DataSchema], string> {
  const multiModalTarget =
    returnType.kind === 'promise' && returnType.element.name === 'Multimodal'
      ? returnType.element
      : returnType.name === 'Multimodal'
        ? returnType
        : null;

  if (
    multiModalTarget &&
    multiModalTarget.name === 'Multimodal' &&
    multiModalTarget.kind === 'array'
  ) {
    const elementType = multiModalTarget.element;

    const multimodalTypes =
      elementType.kind === 'union' ? elementType.unionTypes : [elementType];

    const multiModalDetails = getMultimodalDetails(
      agentClassName,
      multimodalTypes,
    );

    if (Either.isLeft(multiModalDetails)) {
      return Either.left(
        `Failed to get multimodal details: ${multiModalDetails.val}`,
      );
    }

    const multiModalTypeInfos = multiModalDetails.val.map(
      ([name, , typeInfo]) => [name, typeInfo] as [string, TypeInfoInternal],
    );

    const schemaDetails = multiModalDetails.val.map(
      ([name, schema]) => [name, schema] as [string, ElementSchema],
    );

    return Either.right([
      { tag: 'multimodal', val: multiModalTypeInfos },
      {
        tag: 'multimodal',
        val: schemaDetails,
      },
    ]);
  }

  const undefinedSchema = handleUndefinedReturnType(returnType);
  if (Option.isSome(undefinedSchema)) {
    return Either.right([
      { tag: 'analysed', val: tuple(undefined, 'undefined', []) },
      undefinedSchema.val,
    ]);
  }

  const unstructuredText = handleUnstructuredCase(
    returnType,
    'UnstructuredText',
    (t) =>
      Either.map(getTextDescriptor(t), (desc) => ({
        tag: 'unstructured-text',
        val: desc,
      })),
  );
  if (Either.isRight(unstructuredText)) {
    return unstructuredText;
  }

  const unstructuredBinary = handleUnstructuredCase(
    returnType,
    'UnstructuredBinary',
    (t) =>
      Either.map(getBinaryDescriptor(t), (desc) => ({
        tag: 'unstructured-binary',
        val: desc,
      })),
  );

  if (Either.isRight(unstructuredBinary)) {
    return unstructuredBinary;
  }

  const schema: Either.Either<[TypeDetails, ElementSchema], string> =
    Either.map(WitType.fromTsType(returnType, Option.none()), (typeInfo) => {
      const witType = typeInfo[0];
      const analysedType = typeInfo[1];

      return [
        { tag: 'analysed', val: analysedType },
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

function getMultimodalDetails(
  agentClassName: AgentClassName,
  types: Type.Type[],
): Either.Either<[string, ElementSchema, TypeInfoInternal][], string> {
  return Either.all(
    types.map((paramType) => {
      const paramTypeName = paramType.name ?? paramType.kind; // For primitive types, aliases are not preserved

      if (paramTypeName && paramTypeName === 'UnstructuredText') {
        const textDescriptor = getTextDescriptor(paramType);

        if (Either.isLeft(textDescriptor)) {
          return Either.left(
            `Failed to get text descriptor for unstructured-text parameter ${paramTypeName}: ${textDescriptor.val}`,
          );
        }

        let typeInfoInternal: TypeInfoInternal = {
          tag: 'unstructured-text',
          val: textDescriptor.val,
          tsType: paramType,
        };

        const elementSchema: ElementSchema = {
          tag: 'unstructured-text',
          val: textDescriptor.val,
        };

        return Either.right([paramTypeName, elementSchema, typeInfoInternal]);
      }

      if (paramTypeName && paramTypeName === 'UnstructuredBinary') {
        const binaryDescriptor = getBinaryDescriptor(paramType);

        if (Either.isLeft(binaryDescriptor)) {
          return Either.left(
            `Failed to get binary descriptor for unstructured-binary parameter ${paramTypeName}: ${binaryDescriptor.val}`,
          );
        }

        const typeInfoInternal: TypeInfoInternal = {
          tag: 'unstructured-binary',
          val: binaryDescriptor.val,
          tsType: paramType,
        };

        const elementSchema: ElementSchema = {
          tag: 'unstructured-binary',
          val: binaryDescriptor.val,
        };

        return Either.right([paramTypeName, elementSchema, typeInfoInternal]);
      }

      const witType = WitType.fromTsType(paramType, Option.none());

      return Either.map(witType, (typeInfo) => {
        const witType = typeInfo[0];

        const analysedType = typeInfo[1];

        const typeInfoInternal: TypeInfoInternal = {
          tag: 'analysed',
          val: analysedType,
          tsType: paramType,
        };

        const elementSchema: ElementSchema = {
          tag: 'component-model',
          val: witType,
        };
        return [paramTypeName, elementSchema, typeInfoInternal];
      });
    }),
  );
}

function getParameterNameAndElementSchema(
  agentClassName: AgentClassName,
  constructorParamInfos: readonly ConstructorArg[],
): Either.Either<[string, ElementSchema][], string> {
  return Either.all(
    constructorParamInfos.map((paramInfo) => {
      const paramType = paramInfo.type;

      const paramTypeName = paramType.name;

      if (paramTypeName && paramTypeName === 'UnstructuredText') {
        const textDescriptor = getTextDescriptor(paramType);

        if (Either.isLeft(textDescriptor)) {
          return Either.left(
            `Failed to get text descriptor for unstructured-text parameter ${paramInfo.name}: ${textDescriptor.val}`,
          );
        }

        AgentConstructorParamRegistry.setType(agentClassName, paramInfo.name, {
          tag: 'unstructured-text',
          val: textDescriptor.val,
          tsType: paramType,
        });

        const elementSchema: ElementSchema = {
          tag: 'unstructured-text',
          val: textDescriptor.val,
        };

        return Either.right([paramInfo.name, elementSchema]);
      }

      if (paramTypeName && paramTypeName === 'UnstructuredBinary') {
        const binaryDescriptor = getBinaryDescriptor(paramType);

        if (Either.isLeft(binaryDescriptor)) {
          return Either.left(
            `Failed to get binary descriptor for unstructured-binary parameter ${paramInfo.name}: ${binaryDescriptor.val}`,
          );
        }

        AgentConstructorParamRegistry.setType(agentClassName, paramInfo.name, {
          tag: 'unstructured-binary',
          val: binaryDescriptor.val,
          tsType: paramType,
        });

        const elementSchema: ElementSchema = {
          tag: 'unstructured-binary',
          val: binaryDescriptor.val,
        };

        return Either.right([paramInfo.name, elementSchema]);
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

        AgentConstructorParamRegistry.setType(agentClassName, paramInfo.name, {
          tag: 'analysed',
          val: analysedType,
          tsType: paramType,
        });

        const elementSchema: ElementSchema = {
          tag: 'component-model',
          val: witType,
        };
        return [paramInfo.name, elementSchema];
      });
    }),
  );
}

function handleUnstructuredCase(
  returnType: Type.Type,
  typeName: string,
  getElementSchema: (t: Type.Type) => Either.Either<ElementSchema, string>,
): Either.Either<[TypeDetails, DataSchema], string> {
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

  if (typeName === 'UnstructuredText') {
    return Either.right([
      { tag: 'unstructured-text' },
      {
        tag: 'tuple',
        val: [['return-value', elementSchema.val]],
      },
    ]);
  }

  return Either.right([
    { tag: 'unstructured-binary' },
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
    const textDescriptor = getTextDescriptor(parameterType);

    if (Either.isLeft(textDescriptor)) {
      return Either.left(
        `Failed to get text descriptor for unstructured-text parameter ${parameterName}: ${textDescriptor.val}`,
      );
    }

    AgentMethodParamRegistry.setType(
      agentClassName,
      methodName,
      parameterName,
      {
        tag: 'unstructured-text',
        val: textDescriptor.val,
        tsType: parameterType,
      },
    );

    const elementSchema: ElementSchema = {
      tag: 'unstructured-text',
      val: textDescriptor.val,
    };

    return Either.right(elementSchema);
  }

  if (paramTypeName && paramTypeName === 'UnstructuredBinary') {
    const binaryDescriptor = getBinaryDescriptor(parameterType);

    if (Either.isLeft(binaryDescriptor)) {
      return Either.left(
        `Failed to get binary descriptor for unstructured-binary parameter ${parameterName}: ${binaryDescriptor.val}`,
      );
    }

    AgentMethodParamRegistry.setType(
      agentClassName,
      methodName,
      parameterName,
      {
        tag: 'unstructured-binary',
        val: binaryDescriptor.val,
        tsType: parameterType,
      },
    );

    const elementSchema: ElementSchema = {
      tag: 'unstructured-binary',
      val: binaryDescriptor.val,
    };

    return Either.right(elementSchema);
  }

  return Either.map(WitType.fromTsType(parameterType, scope), (typeInfo) => {
    const witType = typeInfo[0];
    const analysedType = typeInfo[1];

    AgentMethodParamRegistry.setType(
      agentClassName,
      methodName,
      parameterName,
      { tag: 'analysed', val: analysedType, tsType: parameterType },
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

function getBinaryDescriptor(
  paramType: Type.Type,
): Either.Either<BinaryDescriptor, string> {
  const mimeTypes = getMimeTypes(paramType);

  if (Either.isLeft(mimeTypes)) {
    return Either.left(`Failed to get mime types: ${mimeTypes.val}`);
  }

  const binaryDescriptor =
    mimeTypes.val.length > 0
      ? {
          restrictions: mimeTypes.val.map((type) => ({
            mimeType: type,
          })),
        }
      : {};

  return Either.right(binaryDescriptor);
}

function getTextDescriptor(
  paramType: Type.Type,
): Either.Either<TextDescriptor, string> {
  const languageCodes = getLanguageCodes(paramType);

  if (Either.isLeft(languageCodes)) {
    return Either.left(`Failed to get language code: ${languageCodes.val}`);
  }

  const textDescriptor: TextDescriptor =
    languageCodes.val.length > 0
      ? {
          restrictions: languageCodes.val.map((code) => ({
            languageCode: code,
          })),
        }
      : {};

  return Either.right(textDescriptor);
}

export function getMimeTypes(type: Type.Type): Either.Either<string[], string> {
  const promiseUnwrappedType = type.kind === 'promise' ? type.element : type;

  if (
    promiseUnwrappedType.name === 'UnstructuredBinary' &&
    promiseUnwrappedType.kind === 'union'
  ) {
    const parameterTypes: Type.Type[] = promiseUnwrappedType.typeParams ?? [];

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
  const promiseUnwrappedType = type.kind === 'promise' ? type.element : type;

  if (
    promiseUnwrappedType.name === 'UnstructuredText' &&
    promiseUnwrappedType.kind === 'union'
  ) {
    const parameterTypes: Type.Type[] = promiseUnwrappedType.typeParams ?? [];

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
