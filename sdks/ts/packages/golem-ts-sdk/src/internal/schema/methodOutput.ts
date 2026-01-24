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
  getMultimodalParamDetails,
  getTextDescriptor,
  isMultimodalType,
} from './helpers';
import {
  getReturnTypeDataSchemaFromTypeInternal,
  TypeInfoInternal,
} from '../typeInfoInternal';
import { AgentMethodRegistry } from '../registry/agentMethodRegistry';

export function resolveMethodReturnDataSchema(
  agentClassName: string,
  methodName: string,
  returnType: Type.Type,
): Either.Either<DataSchema, string> {
  const outputTypeInfoInternal = resolveMethodReturnTypeInfo(returnType);

  if (Either.isLeft(outputTypeInfoInternal)) {
    return outputTypeInfoInternal;
  }

  AgentMethodRegistry.setReturnType(
    agentClassName,
    methodName,
    outputTypeInfoInternal.val,
  );

  return getReturnTypeDataSchemaFromTypeInternal(outputTypeInfoInternal.val);
}

export function resolveMethodReturnTypeInfo(
  returnType: Type.Type,
): Either.Either<TypeInfoInternal, string> {
  const multimodal = tryMultimodal(returnType);

  if (multimodal) return multimodal;

  const voidOrResult = tryVoidLike(returnType);

  if (Either.isLeft(voidOrResult)) return voidOrResult;

  if (Option.isSome(voidOrResult.val)) {
    return getTypeDetailsFromVoidLike(voidOrResult.val.val, returnType);
  }

  const unstructured = tryUnstructured(returnType);

  if (unstructured) return unstructured;

  return mapStandardTsType(returnType);
}

function tryMultimodal(
  returnType: Type.Type,
): Either.Either<TypeInfoInternal, string> | undefined {
  const multimodalOrUndefined =
    returnType.kind === 'promise' && isMultimodalType(returnType.element)
      ? returnType.element
      : isMultimodalType(returnType)
        ? returnType
        : null;

  if (!multimodalOrUndefined || multimodalOrUndefined.kind !== 'array')
    return undefined;

  const details = getMultimodalParamDetails(multimodalOrUndefined.element);

  return Either.map(details, (details) => ({
    tag: 'multimodal',
    tsType: multimodalOrUndefined,
    types: details,
  }));
}

type VoidLike =
  | { kind: 'void'; schema: DataSchema }
  | { kind: 'result-with-void'; analysed: AnalysedType };

function tryVoidLike(
  type: Type.Type,
): Either.Either<Option.Option<VoidLike>, string> {
  switch (type.kind) {
    case 'void':
    case 'undefined':
    case 'null':
      return Either.right(
        Option.some({ kind: 'void', schema: { tag: 'tuple', val: [] } }),
      );

    case 'promise':
      return tryVoidLike(type.element);

    case 'union':
      return tryResultWithVoid(type, type.originalTypeName, type.typeParams);

    default:
      return Either.right(Option.none());
  }
}

function tryResultWithVoid(
  type: Type.Type,
  originalTypename: string | undefined,
  resultTypeParams: Type.Type[],
): Either.Either<Option.Option<VoidLike>, string> {
  const isResultType = type.name === 'Result' || originalTypename === 'Result';
  if (!isResultType || !resultTypeParams) return Either.right(Option.none());

  const [okType, errType] = resultTypeParams;
  const okEmpty = tryEmptyType(okType);
  const errEmpty = tryEmptyType(errType);

  if (!okEmpty && !errEmpty) return Either.right(Option.none());

  const analysedOkEither = okEmpty
    ? Either.right<AnalysedType | undefined, string>(undefined)
    : Either.flatMap(
        WitType.fromTsType(okType, Option.none()),
        ([, analysedOk]) =>
          Either.right<AnalysedType | undefined, string>(analysedOk),
      );

  const analysedErrEither = errEmpty
    ? Either.right<AnalysedType | undefined, string>(undefined)
    : Either.flatMap(
        WitType.fromTsType(errType, Option.none()),
        ([_, analysedErr]) =>
          Either.right<AnalysedType | undefined, string>(analysedErr),
      );

  return Either.flatMap(analysedOkEither, (analysedOk) =>
    Either.flatMap(analysedErrEither, (analysedErr) => {
      const analysedResult: AnalysedType = result(
        undefined,
        { tag: 'inbuilt', okEmptyType: okEmpty, errEmptyType: errEmpty },
        analysedOk ?? undefined,
        analysedErr ?? undefined,
      );

      return Either.right(
        Option.some({
          kind: 'result-with-void',
          analysed: analysedResult,
        }),
      );
    }),
  );
}

function tryEmptyType(type: Type.Type): EmptyType | undefined {
  if (type.kind === 'void') return 'void';
  if (type.kind === 'undefined') return 'undefined';
  if (type.kind === 'null') return 'null';
  return undefined;
}

function getTypeDetailsFromVoidLike(
  resolved: VoidLike,
  tsType: Type.Type,
): Either.Either<TypeInfoInternal, string> {
  switch (resolved.kind) {
    case 'void': {
      const analysed = tuple(undefined, 'undefined', []);
      return Either.right({
        tag: 'analysed',
        val: analysed,
        tsType,
        witType: WitType.fromAnalysedType(analysed),
      });
    }
    case 'result-with-void':
      return Either.right({
        tag: 'analysed',
        val: resolved.analysed,
        tsType,
        witType: WitType.fromAnalysedType(resolved.analysed),
      });
  }
}

function tryUnstructured(
  type: Type.Type,
): Either.Either<TypeInfoInternal, string> | undefined {
  const target =
    type.kind === 'promise' && isUnstructuredType(type.element)
      ? type.element
      : isUnstructuredType(type)
        ? type
        : null;

  if (!target) return undefined;

  if (target.name === 'UnstructuredText') {
    return Either.map(getTextDescriptor(target), (desc) => ({
      tag: 'unstructured-text',
      val: desc,
      tsType: target,
    }));
  }
  if (target.name === 'UnstructuredBinary') {
    return Either.map(getBinaryDescriptor(target), (desc) => ({
      tag: 'unstructured-binary',
      val: desc,
      tsType: target,
    }));
  }

  return undefined;
}

function isUnstructuredType(type: Type.Type): boolean {
  return type.name === 'UnstructuredText' || type.name === 'UnstructuredBinary';
}

function mapStandardTsType(
  type: Type.Type,
): Either.Either<TypeInfoInternal, string> {
  return Either.map(
    WitType.fromTsType(type, Option.none()),
    ([witType, analysed]) => ({
      tag: 'analysed',
      val: analysed,
      witType,
      tsType: type,
    }),
  );
}
