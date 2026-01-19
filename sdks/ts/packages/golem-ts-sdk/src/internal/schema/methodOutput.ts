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
import * as Option from '../../newTypes/option';
import { DataSchema } from 'golem:agent/common';
import * as WitType from '../mapping/types/WitType';
import {
  AnalysedType,
  EmptyType,
  result,
  tuple,
} from '../mapping/types/AnalysedType';
import {
  getBinaryDescriptor,
  getMultimodalDetails,
  getTextDescriptor,
  isNamedMultimodal,
} from './helpers';
import { TypeInfoInternal } from '../typeInfoInternal';

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

    const typeInfoInternal: TypeInfoInternal = {
      tag: 'multimodal',
      tsType: multiModalTarget,
      types: multiModalDetails.val,
    };

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

        return Either.right({
          tag: 'analysed',
          val: voidAnalysedType,
          tsType: returnType,
          witType: witType,
        });
      case 'result-with-void':
        const resultWithVoidWitType = WitType.fromAnalysedType(
          undefinedSchemaVal.analysedType,
        );

        return Either.right({
          tag: 'analysed',
          val: undefinedSchemaVal.analysedType,
          witType: resultWithVoidWitType,
          tsType: returnType,
        });
    }
  }

  const unstructured = handleUnstructuredType(returnType);

  if (unstructured) {
    return unstructured;
  }

  return Either.map(
    WitType.fromTsType(returnType, Option.none()),
    (typeInfo) => {
      const witType = typeInfo[0];
      const analysedType = typeInfo[1];

      return {
        tag: 'analysed',
        val: analysedType,
        witType: witType,
        tsType: returnType,
      };
    },
  );
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

function handleUnstructuredType(
  returnType: Type.Type,
): Either.Either<TypeInfoInternal, string> | undefined {
  const unstructuredTarget =
    returnType.kind === 'promise' &&
    (returnType.element.name === 'UnstructuredText' ||
      returnType.element.name === 'UnstructuredBinary')
      ? returnType.element
      : returnType.name === 'UnstructuredText' ||
          returnType.name === 'UnstructuredBinary'
        ? returnType
        : null;

  if (!unstructuredTarget) {
    return undefined;
  }

  if (unstructuredTarget.name === 'UnstructuredText') {
    return Either.map(getTextDescriptor(unstructuredTarget), (desc) => ({
      tag: 'unstructured-text',
      val: desc,
      tsType: unstructuredTarget,
    }));
  }

  if (unstructuredTarget.name === 'UnstructuredBinary') {
    return Either.map(getBinaryDescriptor(unstructuredTarget), (desc) => ({
      tag: 'unstructured-binary',
      val: desc,
      tsType: unstructuredTarget,
    }));
  }

  return undefined;
}
