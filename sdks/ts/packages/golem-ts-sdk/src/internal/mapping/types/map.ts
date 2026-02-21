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
import { Ctx } from './ctx';
import { AnalysedType, list, tuple } from './analysedType';
import { TypeMapper } from './typeMapper';

type TsType = CoreType.Type;

type MapCtx = Ctx & { type: Extract<TsType, { kind: 'map' }> };

export function handleMap(
  { type }: MapCtx,
  mapper: TypeMapper,
): Either.Either<AnalysedType, string> {
  const key = mapper(type.key, undefined);
  const value = mapper(type.value, undefined);

  return Either.zipWith(key, value, (k, v) =>
    list(type.name, undefined, { keyType: k, valueType: v }, tuple(undefined, undefined, [k, v])),
  );
}
