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
  HttpMountDetails,
  TextDescriptor,
} from 'golem:agent/common';
import * as WitType from './mapping/types/WitType';
import { AgentMethodRegistry } from './registry/agentMethodRegistry';
import {
  ClassMetadata,
  ConstructorArg,
  MethodParams,
} from '@golemcloud/golem-ts-types-core';
import { TypeMappingScope } from './mapping/types/scope';
import { AgentConstructorParamRegistry } from './registry/agentConstructorParamRegistry';
import { AgentMethodParamRegistry } from './registry/agentMethodParamRegistry';
import {
  AnalysedType,
  EmptyType,
  result,
  tuple,
} from './mapping/types/AnalysedType';
import {
  convertTypeInfoToElementSchema,
  getMultimodalDataSchema, getReturnTypeDataSchema,
  TypeInfoInternal,
} from './registry/typeInfoInternal';
import {
  convertAgentMethodNameToKebab,
  convertVariantTypeNameToKebab,
} from './mapping/types/stringFormat';
import { ParameterDetail } from './mapping/values/dataValue';
import { getTaggedUnion, TaggedUnion } from './mapping/types/taggedUnion';
import {
  validateHttpEndpoint,
} from './http/validation';

const MULTIMODAL_TYPE_NAMES = [
  'Multimodal',
  'MultimodalAdvanced',
  'MultimodalCustom',
];

export function getConstructorDataSchema(
  agentClassName: string,
  classType: ClassMetadata,
): Either.Either<DataSchema, string> {
  const constructorParamInfos: readonly ConstructorArg[] =
    classType.constructorArgs;

  if (
    constructorParamInfos.length === 1 &&
    isNamedMultimodal(constructorParamInfos[0].type)
  ) {
    const paramType = constructorParamInfos[0].type;

    if (isNamedMultimodal(paramType) && paramType.kind === 'array') {
      const elementType = paramType.element;

      const multiModalDetails = getMultimodalDetails(elementType);

      if (Either.isLeft(multiModalDetails)) {
        return Either.left(
          `Failed to get multimodal details: ${multiModalDetails.val}`,
        );
      }

      const typeInfoInternal: TypeInfoInternal = {
        tag: 'multimodal',
        tsType: paramType,
        types: multiModalDetails.val,
      }

      const schemaDetails: Either.Either<[string, ElementSchema][], string> = Either.all(multiModalDetails.val.map(
        (parameterDetail) => {
          const elementSchema = convertTypeInfoToElementSchema(parameterDetail.type);

          if (Either.isLeft(elementSchema)) {
            return Either.left(
              `Nested multimodal types are not supported. ${parameterDetail.name}: ${elementSchema.val}`,
            );
          }

          return Either.right([parameterDetail.name, elementSchema.val] as [string, ElementSchema]);
        }
      ));

      if (Either.isLeft(schemaDetails)) {
        return Either.left(schemaDetails.val);
      }

      AgentConstructorParamRegistry.setType(
        agentClassName,
        constructorParamInfos[0].name,
        typeInfoInternal
      );

      return Either.right({
        tag: 'multimodal',
        val: schemaDetails.val,
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
  agentClassName: string,
  httpMountDetails: HttpMountDetails | undefined,
): Either.Either<AgentMethod[], string> {
  if (!classMetadata) {
    return Either.left(`No metadata found for agent class ${agentClassName}`);
  }

  const methodMetadata = Array.from(classMetadata.methods.entries());

  return Either.all(
    methodMetadata.map((methodInfo) => {
      const methodName = methodInfo[0];
      const signature = methodInfo[1];
      const parameters: MethodParams = signature.methodParams;
      const returnType: Type.Type = signature.returnType;

      const methodNameValidation = validateMethodName(methodName);
      if (Either.isLeft(methodNameValidation)) {
        return Either.left(methodNameValidation.val);
      }

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

      const outputTypeInfoEither: Either.Either<
        TypeInfoInternal,
        string
      > = buildOutputSchema(returnType);

      if (Either.isLeft(outputTypeInfoEither)) {
        return Either.left(
          `Failed to construct output schema for method ${methodName} with return type ${returnType.name}: ${outputTypeInfoEither.val}.`,
        );
      }

      const outputTypeInfoInternal = outputTypeInfoEither.val;

      AgentMethodRegistry.setReturnType(agentClassName, methodName, outputTypeInfoInternal);

      const outputSchemaEither = getReturnTypeDataSchema(outputTypeInfoInternal);
      if (Either.isLeft(outputSchemaEither)) {
        return Either.left(
          `Failed to get output data schema for method ${methodName}: ${outputSchemaEither.val}`,
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

      return Either.right({
        name: methodName,
        description: baseMeta.description ?? '',
        promptHint: baseMeta.prompt ?? '',
        inputSchema: inputSchema,
        outputSchema: outputSchema,
        httpEndpoint: baseMeta.httpEndpoint ?? [],
      });
    }),
  );
}

export function buildMethodInputSchema(
  agentClassName: string,
  methodName: string,
  paramTypes: MethodParams,
): Either.Either<DataSchema, string> {
  const paramTypesArray = Array.from(paramTypes);

  if (
    paramTypesArray.length === 1 &&
    isNamedMultimodal(paramTypesArray[0][1])
  ) {
    const paramType = paramTypesArray[0][1];

    if (isNamedMultimodal(paramType) && paramType.kind === 'array') {
      const multiModalDetails = getMultimodalDetails(paramType.element);

      if (Either.isLeft(multiModalDetails)) {
        return Either.left(
          `Failed to get multimodal details: ${multiModalDetails.val}`,
        );
      }

      const typeInfoInternal: TypeInfoInternal = {
        tag: 'multimodal',
        tsType: paramType,
        types: multiModalDetails.val,
      }

      const multimodalDataSchema =
        getMultimodalDataSchema(typeInfoInternal);

      if (Either.isLeft(multimodalDataSchema)) {
        return Either.left(multimodalDataSchema.val);
      }

      AgentMethodParamRegistry.setType(
        agentClassName,
        methodName,
        paramTypesArray[0][0],
        {
          tag: 'multimodal',
          tsType: paramType,
          types: multiModalDetails.val,
        },
      );

      return Either.right(multimodalDataSchema.val);
    } else {
      return Either.left('Multimodal type is not an array');
    }
  } else {
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

    return Either.map(result, (res) => {
      return {
        tag: 'tuple',
        val: res,
      };
    });
  }
}

export function buildOutputSchema(
  returnType: Type.Type,
): Either.Either<TypeInfoInternal, string> {
  const multiModalTarget =
    returnType.kind === 'promise' && isNamedMultimodal(returnType.element)
      ? returnType.element
      : isNamedMultimodal(returnType)
        ? returnType
        : null;

  if (
    multiModalTarget &&
    isNamedMultimodal(multiModalTarget) &&
    multiModalTarget.kind === 'array'
  ) {
    const multiModalDetails = getMultimodalDetails(multiModalTarget.element);

    if (Either.isLeft(multiModalDetails)) {
      return Either.left(
        `Failed to get multimodal details: ${multiModalDetails.val}`,
      );
    }

    const typeInfoInternal : TypeInfoInternal = {
      tag: 'multimodal',
      tsType: multiModalTarget,
      types: multiModalDetails.val,
    }

    return Either.right(typeInfoInternal);

  }

  const undefinedSchema = handleVoidReturnType(returnType);

  if (Either.isLeft(undefinedSchema)) {
    return Either.left(
      `Failed to handle void return type: ${undefinedSchema.val}`,
    );
  }

  if (Option.isSome(undefinedSchema.val)) {
    const undefinedSchemaVal = undefinedSchema.val.val;

    switch (undefinedSchemaVal.kind) {
      case 'void':
        const voidAnalysedType = tuple(undefined, 'undefined', []);
        const witType = WitType.fromAnalysedType(voidAnalysedType);

        return Either.right(
          { tag: 'analysed',  val: voidAnalysedType, tsType: returnType, witType: witType },
        );
      case 'result-with-void':
        const resultWithVoidWitType =
          WitType.fromAnalysedType(undefinedSchemaVal.analysedType);

        return Either.right(
          { tag: 'analysed', val: undefinedSchemaVal.analysedType, witType: resultWithVoidWitType, tsType: returnType }
        );
    }
  }

  const unstructured = handleUnstructuredType(
    returnType,
  );

  if (unstructured) {
    return unstructured;
  }

   return Either.map(WitType.fromTsType(returnType, Option.none()), (typeInfo) => {
      const witType = typeInfo[0];
      const analysedType = typeInfo[1];

      return { tag: 'analysed', val: analysedType, witType: witType, tsType: returnType };
    });
}

function getMultimodalDetails(
  type: Type.Type,
): Either.Either<ParameterDetail[], string> {
  const multimodalTypes =
    type.kind === 'union'
      ? getTaggedUnion(type.unionTypes)
      : getTaggedUnion([type]);

  if (Either.isLeft(multimodalTypes)) {
    return Either.left(
      `failed to generate the multimodal schema: ${multimodalTypes.val}`,
    );
  }

  const taggedUnionOpt = multimodalTypes.val;

  if (Option.isNone(taggedUnionOpt)) {
    return Either.left(
      `multimodal type is not a tagged union: ${multimodalTypes.val}. Expected an object with a literal 'tag' and 'val' property`,
    );
  }

  const taggedTypes = TaggedUnion.getTaggedTypes(taggedUnionOpt.val);

  return Either.all(
    taggedTypes.map((taggedTypeMetadata) => {
      const paramTypeOpt = taggedTypeMetadata.valueType;

      if (Option.isNone(paramTypeOpt)) {
        return Either.left(
          `Multimodal types should have a value associated with the tag ${taggedTypeMetadata.tagLiteralName}`,
        );
      }

      const tagName = taggedTypeMetadata.tagLiteralName;

      const valName = paramTypeOpt.val[0];

      if (valName !== 'val') {
        return Either.left(
          `The value associated with the tag ${tagName} should be named 'val', found '${valName}' instead`,
        );
      }

      const paramType = paramTypeOpt.val[1];

      const typeName = paramType.name;

      if (typeName && typeName === 'UnstructuredText') {
        const textDescriptor = getTextDescriptor(paramType);

        if (Either.isLeft(textDescriptor)) {
          return Either.left(
            `Failed to get text descriptor for unstructured-text parameter ${tagName}: ${textDescriptor.val}`,
          );
        }

        let typeInfoInternal: TypeInfoInternal = {
          tag: 'unstructured-text',
          val: textDescriptor.val,
          tsType: paramType,
        };

        return Either.right({
          name: convertVariantTypeNameToKebab(tagName),
          type: typeInfoInternal,
        });
      }

      if (typeName && typeName === 'UnstructuredBinary') {
        const binaryDescriptor = getBinaryDescriptor(paramType);

        if (Either.isLeft(binaryDescriptor)) {
          return Either.left(
            `Failed to get binary descriptor for unstructured-binary parameter ${tagName}: ${binaryDescriptor.val}`,
          );
        }

        const typeInfoInternal: TypeInfoInternal = {
          tag: 'unstructured-binary',
          val: binaryDescriptor.val,
          tsType: paramType,
        };

        return Either.right({
          name: convertVariantTypeNameToKebab(tagName),
          type: typeInfoInternal,
        });
      }

      const witType = WitType.fromTsType(paramType, Option.none());

      return Either.map(witType, (typeInfo) => {
        const witType = typeInfo[0];

        const analysedType = typeInfo[1];

        const typeInfoInternal: TypeInfoInternal = {
          tag: 'analysed',
          val: analysedType,
          tsType: paramType,
          witType: witType,
        };

        return {
          name: convertVariantTypeNameToKebab(tagName),
          type: typeInfoInternal,
        };
      });
    }),
  );
}

function getParameterNameAndElementSchema(
  agentClassName: string,
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
            agentClassName,
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
          witType: witType,
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

function handleUnstructuredType(
  returnType: Type.Type,
): Either.Either<TypeInfoInternal, string> | undefined {

  const unstructuredTarget =
    returnType.kind === 'promise' && (returnType.element.name === 'UnstructuredText' || returnType.element.name === 'UnstructuredBinary')
      ? returnType.element
      : (returnType.name === 'UnstructuredText' || returnType.name === 'UnstructuredBinary')
        ? returnType
        : null;

  if (!unstructuredTarget) {
    return undefined
  }

  if (unstructuredTarget.name === 'UnstructuredText') {
    return Either.map(getTextDescriptor(unstructuredTarget), (desc) => ({
      tag: 'unstructured-text',
      val: desc,
      tsType: unstructuredTarget
    }))
  }


  if (unstructuredTarget.name === 'UnstructuredBinary') {
    return Either.map(getBinaryDescriptor(unstructuredTarget), (desc) => ({
      tag: 'unstructured-binary',
      val: desc,
      tsType: unstructuredTarget
    }))
  }

  return undefined;
}

function checkUnstructuredType(
  returnType: Type.Type,
  typeName: string,
  getTypeInfoInternal: (t: Type.Type) => Either.Either<TypeInfoInternal, string>,
): Either.Either<[TypeInfoInternal, DataSchema], string> {
  const target =
    returnType.kind === 'promise' && returnType.element.name === typeName
      ? returnType.element
      : returnType.name === typeName
        ? returnType
        : null;

  if (!target) {
    return Either.left('not-special-case');
  }


  const typeInfoInternalEither = getTypeInfoInternal(target);

  if (typeName === 'UnstructuredText') {
    return Either.right([
      { tag: 'unstructured-text', val:  },
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

export function convertToElementSchema(
  agentClassName: string,
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

// To handle void, undefined, null return types or Result with void/undefined/null on either side
type ReturnTypeWithVoid =
  | { kind: 'void'; dataSchema: DataSchema }
  | { kind: 'result-with-void'; analysedType: AnalysedType };

function handleVoidReturnType(
  returnType: Type.Type,
): Either.Either<Option.Option<ReturnTypeWithVoid>, string> {
  switch (returnType.kind) {
    case 'null':
      return Either.right(
        Option.some({
          kind: 'void',
          dataSchema: {
            tag: 'tuple',
            val: [],
          },
        }),
      );

    case 'undefined':
      return Either.right(
        Option.some({
          kind: 'void',
          dataSchema: {
            tag: 'tuple',
            val: [],
          },
        }),
      );

    case 'void':
      return Either.right(
        Option.some({
          kind: 'void',
          dataSchema: {
            tag: 'tuple',
            val: [],
          },
        }),
      );

    case 'promise':
      const elementType = returnType.element;
      return handleVoidReturnType(elementType);

    // Special handling for union types that might include void/undefined/null
    case 'union':
      const typeName = returnType.name;
      const originalTypeName = returnType.originalTypeName;
      const unionTypes = returnType.unionTypes;
      const isResult = typeName === 'Result' || originalTypeName === 'Result';

      if (
        isResult &&
        unionTypes.length === 2 &&
        unionTypes[0].name === 'Ok' &&
        unionTypes[1].name === 'Err'
      ) {
        const resultTypeParams = returnType.typeParams;

        const okType = resultTypeParams[0];
        const errType = resultTypeParams[1];

        const okEmptyType: EmptyType | undefined =
          okType.kind === 'void'
            ? 'void'
            : okType.kind === 'undefined'
              ? 'undefined'
              : okType.kind === 'null'
                ? 'null'
                : undefined;

        const errEmptyType: EmptyType | undefined =
          errType.kind === 'void'
            ? 'void'
            : errType.kind === 'undefined'
              ? 'undefined'
              : errType.kind === 'null'
                ? 'null'
                : undefined;

        const isOkVoid = okEmptyType !== undefined;

        const isErrVoid = errEmptyType !== undefined;

        if (isOkVoid && isErrVoid) {
          return Either.right(
            Option.some({
              kind: 'result-with-void',
              analysedType: result(
                undefined,
                {
                  tag: 'inbuilt',
                  okEmptyType: okEmptyType,
                  errEmptyType: errEmptyType,
                },
                undefined,
                undefined,
              ),
            }),
          );
        }

        if (isOkVoid) {
          const errAnalysedTypeEither = WitType.fromTsType(
            errType,
            Option.none(),
          );

          if (Either.isLeft(errAnalysedTypeEither)) {
            return errAnalysedTypeEither;
          }

          const errAnalysedType = errAnalysedTypeEither.val[1];

          return Either.right(
            Option.some({
              kind: 'result-with-void',
              analysedType: result(
                undefined,
                {
                  tag: 'inbuilt',
                  okEmptyType: okEmptyType,
                  errEmptyType: errEmptyType,
                },
                undefined,
                errAnalysedType,
              ),
            }),
          );
        }

        if (isErrVoid) {
          const okAnalysedTypeEither = WitType.fromTsType(
            okType,
            Option.none(),
          );

          if (Either.isLeft(okAnalysedTypeEither)) {
            return okAnalysedTypeEither;
          }

          const okAnalysedType = okAnalysedTypeEither.val[1];

          return Either.right(
            Option.some({
              kind: 'result-with-void',
              analysedType: result(
                undefined,
                {
                  tag: 'inbuilt',
                  okEmptyType: okEmptyType,
                  errEmptyType: errEmptyType,
                },
                okAnalysedType,
                undefined,
              ),
            }),
          );
        }

        return Either.right(Option.none());
      }

      return Either.right(Option.none());

    default:
      return Either.right(Option.none());
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
    const unstructuredBinaryTypeParameters: Type.Type[] =
      promiseUnwrappedType.typeParams ?? [];

    if (unstructuredBinaryTypeParameters.length === 0) {
      return Either.right([]);
    }

    const unstructuredBinaryTypeParameter: Type.Type =
      unstructuredBinaryTypeParameters[0];

    if (unstructuredBinaryTypeParameter.kind === 'tuple') {
      const elem = unstructuredBinaryTypeParameter.elements;

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
    } else if (unstructuredBinaryTypeParameter.kind === 'string') {
      // If the type parameter is of `type` string, it implies, we return an empty set of mime-types,
      // and the absence of restrictions would result in allowing any mime type
      return Either.right([]);
    } else {
      return Either.left(
        'unknown parameter type for UnstructuredBinary' +
          unstructuredBinaryTypeParameter.kind,
      );
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

function isNamedMultimodal(type: Type.Type): boolean {
  if (type.name) {
    return MULTIMODAL_TYPE_NAMES.includes(type.name);
  }
  return false;
}

function validateMethodName(methodName: string): Either.Either<void, string> {
  if (methodName.includes('$')) {
    return Either.left(
      `Invalid method name \`${methodName}\`: cannot contain '\$'`,
    );
  }

  const kebabMethodName = convertAgentMethodNameToKebab(methodName);
  if (
    kebabMethodName === 'initialize' ||
    kebabMethodName === 'get-definition'
  ) {
    return Either.left(
      `Invalid method name \`${methodName}\`: reserved method name`,
    );
  }

  return Either.right(undefined);
}
