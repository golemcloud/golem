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
import { AnalysedType } from './analysedType';
import { Ctx } from './ctx';
import { TypeMapper } from './typeMapper';

type TsType = CoreType.Type;

type UnresolvedCtx = Ctx & {
  type: Extract<TsType, { kind: 'unresolved-type' }>;
};

export function handleUnresolved(
  { type }: UnresolvedCtx,
  _mapper: TypeMapper,
): Either.Either<AnalysedType, string> {
  return Either.left(`Failed to resolve type for \`${type.text}\`: ${type.error}`);
}
