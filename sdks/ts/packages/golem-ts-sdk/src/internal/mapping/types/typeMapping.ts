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

import { Type as CoreType } from '@golemcloud/golem-ts-types-core';
import * as Either from "../../../newTypes/either";
import * as Option from "../../../newTypes/option";
import { TypeMappingScope } from './scope';
import { callHandler } from './handlers';
import { ctx } from './ctx';
import { AnalysedType, option } from './analysedType';

type TsType = CoreType.Type;

export function fromTsType(tsType: TsType, scope: Option.Option<TypeMappingScope>): Either.Either<AnalysedType, string> {
  const result =
    fromTsTypeInternal(tsType, scope);

  if (Option.isSome(scope) && TypeMappingScope.isOptional(scope.val)) {
    return Either.map(result, (analysedType) => {

      if (analysedType.kind === 'option' && analysedType.emptyType !== 'question-mark') {
        return analysedType;
      }

      return option(undefined, "question-mark", analysedType)
    })
  }

  return result
}

export function fromTsTypeInternal(type: TsType, scope: Option.Option<TypeMappingScope>): Either.Either<AnalysedType, string> {
  const rejected = rejectBoxedTypes(type);
  if (Either.isLeft(rejected)) return rejected;

  return callHandler(type.kind, ctx(type, scope));
}


function rejectBoxedTypes(type: TsType): Either.Either<never, string> {
  switch (type.name) {
    case "String":  return Either.left("Unsupported type `String`, use `string` instead");
    case "Boolean": return Either.left("Unsupported type `Boolean`, use `boolean` instead");
    case "BigInt":  return Either.left("Unsupported type `BigInt`, use `bigint` instead");
    case "Number":  return Either.left("Unsupported type `Number`, use `number` instead");
    case "Symbol":  return Either.left("Unsupported type `Symbol`, use `string` if possible");
    case "Date":    return Either.left("Unsupported type `Date`. Use a `string` if possible");
    case "RegExp":  return Either.left("Unsupported type `RegExp`. Use a `string` if possible");
  }
  return Either.right(undefined as never);
}


