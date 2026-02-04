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
import * as Either from '../../../newTypes/either';
import { TypeScope } from './scope';
import { callHandler } from './handlers';
import { createCtx } from './ctx';
import { AnalysedType, option } from './analysedType';
import { TypeMapper } from './typeMapper';

type TsType = CoreType.Type;

export const typeMapper: TypeMapper = (tsType: TsType, scope: TypeScope | undefined) => {
  const result = mapTypeInternal(tsType, scope);

  if (scope && TypeScope.isQuestionMarkOptional(scope)) {
    return Either.map(result, (analysedType) => {
      if (analysedType.kind === 'option' && analysedType.emptyType !== 'question-mark') {
        return analysedType;
      }

      return option(undefined, 'question-mark', analysedType);
    });
  }

  return result;
};

export function mapTypeInternal(
  type: TsType,
  scope: TypeScope | undefined,
): Either.Either<AnalysedType, string> {
  const rejected = rejectBoxedTypes(type);
  if (Either.isLeft(rejected)) return rejected;

  return callHandler(type.kind, createCtx(type, scope), typeMapper);
}

function rejectBoxedTypes(type: TsType): Either.Either<never, string> {
  switch (type.name) {
    case 'String':
      return Either.left('Unsupported type `String`, use `string` instead');
    case 'Boolean':
      return Either.left('Unsupported type `Boolean`, use `boolean` instead');
    case 'BigInt':
      return Either.left('Unsupported type `BigInt`, use `bigint` instead');
    case 'Number':
      return Either.left('Unsupported type `Number`, use `number` instead');
    case 'Symbol':
      return Either.left('Unsupported type `Symbol`, use `string` if possible');
    case 'Date':
      return Either.left('Unsupported type `Date`. Use a `string` if possible');
    case 'RegExp':
      return Either.left('Unsupported type `RegExp`. Use a `string` if possible');
  }
  return Either.right(undefined as never);
}
