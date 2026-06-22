// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
import { MultimodalCase, RuntimeTypeInfo } from '../typeInfoInternal';
import { mapTsTypeToResolvedGraph } from '../mapping/types/resolvedMapper';
import { TypeScope } from '../mapping/types/scope';
import { tryTaggedUnion, TaggedUnion } from '../mapping/types/taggedUnion';

const MULTIMODAL_TYPE_NAMES = ['Multimodal', 'MultimodalAdvanced', 'MultimodalCustom'];

export function isMultimodalType(type: Type.Type): boolean {
  if (type.name) {
    return MULTIMODAL_TYPE_NAMES.includes(type.name);
  }
  return false;
}

function isPrincipalType(type: Type.Type): boolean {
  return type.kind === 'principal' || type.name === 'Principal';
}

/**
 * Resolve a constructor/method parameter's TypeScript type into its schema-native
 * {@link RuntimeTypeInfo}. Auto-injected (`principal`) and `config` parameters are
 * recognised here too; the caller decides how they participate in the input schema.
 */
export function resolveParamType(
  scope: TypeScope | undefined,
  type: Type.Type,
): Either.Either<RuntimeTypeInfo, string> {
  if (isPrincipalType(type)) {
    return Either.right({ tag: 'principal', tsType: type });
  }
  if (type.kind === 'config') {
    return Either.right({ tag: 'config', tsType: type as Type.Type & { kind: 'config' } });
  }
  return resolveModalityOrSchemaType(scope, type, true);
}

/**
 * Resolve a type that may be a rich modality (unstructured text/binary), a
 * multimodal list, or a plain schema type. Used for top-level params (when
 * `allowMultimodal` is `true`) and for the individual cases of a multimodal
 * parameter (when `false`).
 */
function resolveModalityOrSchemaType(
  scope: TypeScope | undefined,
  type: Type.Type,
  allowMultimodal: boolean,
): Either.Either<RuntimeTypeInfo, string> {
  if (type.name === 'UnstructuredText') {
    return Either.map(getLanguageCodes(type), (languages) => ({
      tag: 'unstructured-text',
      languages,
      tsType: type,
    }));
  }
  if (type.name === 'UnstructuredBinary') {
    return Either.map(getMimeTypes(type), (mimeTypes) => ({
      tag: 'unstructured-binary',
      mimeTypes,
      tsType: type,
    }));
  }
  if (allowMultimodal && isMultimodalType(type)) {
    return resolveMultimodalType(type);
  }
  return Either.map(mapTsTypeToResolvedGraph(type, scope), (graph) => ({
    tag: 'schema',
    graph,
    tsType: type,
  }));
}

function resolveMultimodalType(type: Type.Type): Either.Either<RuntimeTypeInfo, string> {
  if (type.kind !== 'array') {
    return Either.left('Multimodal type is not an array');
  }
  return Either.map(getMultimodalCases(type.element), (cases) => ({
    tag: 'multimodal',
    cases,
    tsType: type,
  }));
}

/**
 * Resolve the cases of a multimodal element type (a tagged union where each
 * `{ tag, val }` case's `val` is a modality: unstructured text/binary or a
 * plain schema value). Never itself multimodal/principal/config.
 */
export function getMultimodalCases(type: Type.Type): Either.Either<MultimodalCase[], string> {
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
      const tagName = taggedTypeMetadata.tagLiteralName;

      if (!paramTypeOpt) {
        return Either.left(
          `Multimodal types should have a value associated with the tag ${tagName}`,
        );
      }

      const [valName, paramType] = paramTypeOpt;

      if (valName !== 'val') {
        return Either.left(
          `The value associated with the tag ${tagName} should be named 'val', found '${valName}' instead`,
        );
      }

      return Either.map(resolveModalityOrSchemaType(undefined, paramType, false), (modality) => ({
        name: tagName,
        type: modality,
      }));
    }),
  );
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

  if (methodName === 'initialize' || methodName === 'getDefinition') {
    return Either.left(`Invalid method name \`${methodName}\`: reserved method name`);
  }

  return Either.right(undefined);
}
