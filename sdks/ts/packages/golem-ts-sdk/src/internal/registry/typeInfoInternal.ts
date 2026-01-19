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
import { AnalysedType } from '../mapping/types/AnalysedType';
import { Type } from '@golemcloud/golem-ts-types-core';
import { ParameterDetail } from '../mapping/values/dataValue';
import * as Either from '../../newTypes/either';

// For all types except unstructured-*, `AnalysedType` has the max details.
// There is no AnalysedType for unstructured-text/binary
export type TypeInfoInternal =
  | { tag: 'analysed'; val: AnalysedType; witType: WitType; tsType: Type.Type }
  | { tag: 'unstructured-text'; val: TextDescriptor; tsType: Type.Type }
  | { tag: 'unstructured-binary'; val: BinaryDescriptor; tsType: Type.Type }
  | {
      tag: 'multimodal';
      types: ParameterDetail[];
      tsType: Type.Type;
    };

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
    case 'multimodal':
      return Either.left(
        'Cannot convert multimodal type information to ElementSchema',
      );
  }
}

export function getMultimodalDataSchema(
  typeInfoInternal: TypeInfoInternal,
): Either.Either<DataSchema, string> {
  switch (typeInfoInternal.tag) {
    case 'unstructured-text':
      return {
        tag: 'left',
        val: 'cannot get multimodal DataSchema from unstructured-text type info',
      };
    case 'analysed':
      return Either.left(
        'cannot get multimodal DataSchema from analysed type info',
      );
    case 'unstructured-binary':
      return Either.left(
        'cannot get multimodal DataSchema from unstructured-binary type info',
      );
    case 'multimodal':
      const parameterDetails = typeInfoInternal.types;

      const schemaDetails = Either.all(
        parameterDetails.map((parameterDetail) => {
          const elementSchema = convertTypeInfoToElementSchema(
            parameterDetail.type,
          );

          if (Either.isLeft(elementSchema)) {
            return Either.left(
              `Nested multimodal types are not supported. ${parameterDetail.name}: ${elementSchema.val}`,
            );
          }

          return Either.right([parameterDetail.name, elementSchema.val] as [
            string,
            ElementSchema,
          ]);
        }),
      );

      return Either.map(schemaDetails, (details) => ({
        tag: 'multimodal',
        val: details,
      }));
  }
}

export function getReturnTypeDataSchema(
  typeInfoInternal: TypeInfoInternal,
): Either.Either<DataSchema, string> {
  switch (typeInfoInternal.tag) {
    case 'unstructured-text':
      return Either.right({
        tag: 'tuple',
        val: [
          [
            'return-type',
            {
              tag: 'unstructured-text',
              val: typeInfoInternal.val,
            },
          ],
        ],
      });
    case 'analysed':
      return Either.right({
        tag: 'tuple',
        val: [
          [
            'return-type',
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
            'return-type',
            {
              tag: 'unstructured-binary',
              val: typeInfoInternal.val,
            },
          ],
        ],
      });
    case 'multimodal':
      return getMultimodalDataSchema(typeInfoInternal);
  }
}
