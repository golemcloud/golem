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
import { isNumberString, trimQuotes } from './stringFormat';
import { AnalysedType, bool, enum_ } from './analysedType';
import { Ctx } from './ctx';
import { TypeMapper } from './typeMapper';

type TsType = CoreType.Type;

type LiteralCtx = Ctx & { type: Extract<TsType, { kind: 'literal' }> };

export function handleLiteral(
  { type }: LiteralCtx,
  _mapper: TypeMapper,
): Either.Either<AnalysedType, string> {
  const literalName = type.literalValue;

  if (literalName) {
    if (literalName === 'true' || literalName === 'false') {
      return Either.right(bool());
    }
    if (isNumberString(literalName)) {
      return Either.left('Literals of number type are not supported');
    }
    return Either.right(enum_(type.name, [trimQuotes(literalName)]));
  } else {
    return Either.left(
      `internal error: failed to retrieve the literal value from type of kind ${type.kind}`,
    );
  }
}
