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
import { BinaryDescriptor, TextDescriptor } from 'golem:agent/common@1.5.0';
import * as WitType from '../mapping/types/WitType';
import { TypeInfoInternal } from '../typeInfoInternal';
import {
  convertAgentMethodNameToKebab,
  convertVariantTypeNameToKebab,
} from '../mapping/types/stringFormat';
import { ParameterDetail } from '../mapping/values/dataValue';
import { tryTaggedUnion, TaggedUnion } from '../mapping/types/taggedUnion';

const MULTIMODAL_TYPE_NAMES = ['Multimodal', 'MultimodalAdvanced', 'MultimodalCustom'];

export function isMultimodalType(type: Type.Type): boolean {
  if (type.name) {
    return MULTIMODAL_TYPE_NAMES.includes(type.name);
  }
  return false;
}

export function getMultimodalParamDetails(
  type: Type.Type,
): Either.Either<ParameterDetail[], string> {
  const multimodalTypes =
    type.kind === 'union' ? tryTaggedUnion(type.unionTypes) : tryTaggedUnion([type]);

  if (Either.isLeft(multimodalTypes)) {
    return Either.left(`failed to generate the multimodal schema: ${multimodalTypes.val}`);
  }

  const taggedUnion = multimodalTypes.val;

  if (!taggedUnion) {
    return Either.left(
      `multimodal type is not a tagged union: ${multimodalTypes.val}. Expected an object with a literal 'tag' and 'val' property`,
    );
  }

  const taggedTypes = TaggedUnion.getTaggedTypes(taggedUnion);

  return Either.all(
    taggedTypes.map((taggedTypeMetadata) => {
      const paramTypeOpt = taggedTypeMetadata.valueType;

      if (!paramTypeOpt) {
        return Either.left(
          `Multimodal types should have a value associated with the tag ${taggedTypeMetadata.tagLiteralName}`,
        );
      }

      const tagName = taggedTypeMetadata.tagLiteralName;

      const [valName, paramType] = paramTypeOpt;

      if (valName !== 'val') {
        return Either.left(
          `The value associated with the tag ${tagName} should be named 'val', found '${valName}' instead`,
        );
      }

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

      const witType = WitType.fromTsType(paramType, undefined);

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

export function getTextDescriptor(paramType: Type.Type): Either.Either<TextDescriptor, string> {
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

export function getBinaryDescriptor(paramType: Type.Type): Either.Either<BinaryDescriptor, string> {
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

export function getMimeTypes(type: Type.Type): Either.Either<string[], string> {
  const promiseUnwrappedType = type.kind === 'promise' ? type.element : type;

  if (promiseUnwrappedType.name === 'UnstructuredBinary' && promiseUnwrappedType.kind === 'union') {
    const unstructuredBinaryTypeParameters: Type.Type[] = promiseUnwrappedType.typeParams ?? [];

    if (unstructuredBinaryTypeParameters.length === 0) {
      return Either.right([]);
    }

    const unstructuredBinaryTypeParameter: Type.Type = unstructuredBinaryTypeParameters[0];

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
        'unknown parameter type for UnstructuredBinary' + unstructuredBinaryTypeParameter.kind,
      );
    }
  }

  return Either.left(`Type mismisatch. Expected UnstructuredBinary, Found ${type.name}`);
}

export function getLanguageCodes(type: Type.Type): Either.Either<string[], string> {
  const promiseUnwrappedType = type.kind === 'promise' ? type.element : type;

  if (promiseUnwrappedType.name === 'UnstructuredText' && promiseUnwrappedType.kind === 'union') {
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

  return Either.left(`Type mismatch. Expected UnstructuredText, Found ${type.name}`);
}

export function validateMethodName(methodName: string): Either.Either<void, string> {
  if (methodName.includes('$')) {
    return Either.left(`Invalid method name \`${methodName}\`: cannot contain '\$'`);
  }

  const kebabMethodName = convertAgentMethodNameToKebab(methodName);
  if (kebabMethodName === 'initialize' || kebabMethodName === 'get-definition') {
    return Either.left(`Invalid method name \`${methodName}\`: reserved method name`);
  }

  return Either.right(undefined);
}
