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

import {
  BinaryDescriptor,
  DataSchema,
  ElementSchema,
  TextDescriptor,
  WitType,
} from 'golem:agent/common';
import { Type } from '@golemcloud/golem-ts-types-core';
import { ParameterDetail } from './mapping/values/dataValue';
import * as Either from '../newTypes/either';
import { AnalysedType } from './mapping/types/analysedType';

// An internal representation of a type
// This type can represent the type of a constructor parameter,
// or a method parameter or a method output. Note that `TypeInfoInternal`
// cannot represent an entire `DataSchema`, only individual elements that can be part of a `DataSchema`.
// However, `TypeInfoInternal` is enough to retrieve the `DataSchema` for method output, or a multimodal constructor/method parameter.
export type TypeInfoInternal =
  | { tag: 'analysed'; val: AnalysedType; witType: WitType; tsType: Type.Type }
  | { tag: 'unstructured-text'; val: TextDescriptor; tsType: Type.Type }
  | { tag: 'unstructured-binary'; val: BinaryDescriptor; tsType: Type.Type }
  | { tag: 'principal'; tsType: Type.Type }
  | {
      tag: 'multimodal';
      types: ParameterDetail[];
      tsType: Type.Type;
    };

export function isOptionalWithQuestionMark(typeInfoInternal: TypeInfoInternal): boolean {
  if (typeInfoInternal.tsType.kind === 'union') {
    return typeInfoInternal.tsType.unionTypes.some(
      (t) => t.kind === 'undefined' || t.kind === 'null',
    );
  }

  return false;
}

export function isEmptyType(typeInfoInternal: TypeInfoInternal): boolean {
  if (typeInfoInternal.tag === 'analysed') {
    const analysed = typeInfoInternal.val;
    if (analysed.kind === 'tuple' && analysed.emptyType) {
      return true;
    }
  }

  return false;
}

// Except for multimodal, all types can be converted to ElementSchema
export function convertTypeInfoToElementSchema(
  typeInfoInternal: TypeInfoInternal,
): Either.Either<ElementSchema, string> {
  switch (typeInfoInternal.tag) {
    case 'unstructured-text':
      return Either.right({
        tag: 'unstructured-text',
        val: typeInfoInternal.val,
      });
    case 'analysed':
      return Either.right({
        tag: 'component-model',
        val: typeInfoInternal.witType,
      });
    case 'unstructured-binary':
      return Either.right({
        tag: 'unstructured-binary',
        val: typeInfoInternal.val,
      });
    case 'principal':
      return Either.left('Cannot convert `Principal` type information to ElementSchema');
    case 'multimodal':
      return Either.left('Cannot convert multimodal type information to ElementSchema');
  }
}

// It is possible to get an entire `DataSchema` if the typeInfoInternal is `Multimodal`.
// In other cases, it is not possible to get a full DataSchema from a single TypeInfoInternal, because
// DataSchema can represent tuples and other composite types, while TypeInfoInternal represents only individual elements
export function getMultimodalDataSchemaFromTypeInternal(
  typeInfoInternal: TypeInfoInternal,
): Either.Either<DataSchema, string> {
  switch (typeInfoInternal.tag) {
    case 'unstructured-text':
      return {
        tag: 'left',
        val: 'cannot get multimodal DataSchema from unstructured-text type info',
      };
    case 'analysed':
      return Either.left('cannot get multimodal DataSchema from analysed type info');
    case 'unstructured-binary':
      return Either.left('cannot get multimodal DataSchema from unstructured-binary type info');
    case 'principal':
      return Either.left('cannot get multimodal DataSchema from principal type info');
    case 'multimodal':
      const parameterDetails = typeInfoInternal.types;

      const schemaDetails = Either.all(
        parameterDetails.map((parameterDetail) => {
          const elementSchema = convertTypeInfoToElementSchema(parameterDetail.type);

          if (Either.isLeft(elementSchema)) {
            return Either.left(
              `Nested multimodal types are not supported. ${parameterDetail.name}: ${elementSchema.val}`,
            );
          }

          return Either.right([parameterDetail.name, elementSchema.val] as [string, ElementSchema]);
        }),
      );

      return Either.map(schemaDetails, (details) => ({
        tag: 'multimodal',
        val: details,
      }));
  }
}

// It is possible to get an entire `DataSchema` for method return type from a single TypeInfoInternal
// because method return type is always a single element. The DataSchema for the return type is always a tuple with a single element
export function getReturnTypeDataSchemaFromTypeInternal(
  typeInfoInternal: TypeInfoInternal,
): Either.Either<DataSchema, string> {
  switch (typeInfoInternal.tag) {
    case 'unstructured-text':
      return Either.right({
        tag: 'tuple',
        val: [
          [
            'return-value',
            {
              tag: 'unstructured-text',
              val: typeInfoInternal.val,
            },
          ],
        ],
      });
    case 'analysed':
      const analysed = typeInfoInternal.val;
      // If the return type is a void, then the data schema is an empty tuple
      if (analysed.kind === 'tuple' && analysed.emptyType) {
        return Either.right({
          tag: 'tuple',
          val: [],
        });
      }

      return Either.right({
        tag: 'tuple',
        val: [
          [
            'return-value',
            {
              tag: 'component-model',
              val: typeInfoInternal.witType,
            },
          ],
        ],
      });

    case 'unstructured-binary':
      return Either.right({
        tag: 'tuple',
        val: [
          [
            'return-value',
            {
              tag: 'unstructured-binary',
              val: typeInfoInternal.val,
            },
          ],
        ],
      });
    case 'principal':
      return Either.left('Principal cannot be used as a method return type');
    case 'multimodal':
      return getMultimodalDataSchemaFromTypeInternal(typeInfoInternal);
  }
}
