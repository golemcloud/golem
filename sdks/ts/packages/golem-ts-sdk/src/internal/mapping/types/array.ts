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
import { AnalysedType, f32, f64, list, s16, s32, s64, s8, u16, u32, u64, u8 } from './analysedType';
import { TypeMapper } from './typeMapper';

type TsType = CoreType.Type;
type ArrayCtx = Ctx & { type: Extract<TsType, { kind: 'array' }> };

const TYPED_ARRAYS: Record<string, () => AnalysedType> = {
  Float64Array: () => list(undefined, 'f64', undefined, f64()),
  Float32Array: () => list(undefined, 'f32', undefined, f32()),
  Int8Array: () => list(undefined, 'i8', undefined, s8()),
  Uint8Array: () => list(undefined, 'u8', undefined, u8()),
  Int16Array: () => list(undefined, 'i16', undefined, s16()),
  Uint16Array: () => list(undefined, 'u16', undefined, u16()),
  Int32Array: () => list(undefined, 'i32', undefined, s32()),
  Uint32Array: () => list(undefined, 'u32', undefined, u32()),
  BigInt64Array: () => list(undefined, 'big-i64', undefined, s64(true)),
  BigUint64Array: () => list(undefined, 'big-u64', undefined, u64(true)),
};

export function handleArray(
  { type }: ArrayCtx,
  mapper: TypeMapper,
): Either.Either<AnalysedType, string> {
  const name = type.name;

  if (name) {
    const typed = TYPED_ARRAYS[name];
    if (typed) {
      return Either.right(typed());
    }
  }

  const arrayElementType = type.element;

  // For an array type, we don't care about the scope of this element type, hence undefined
  const elemType = mapper(arrayElementType, undefined);

  return Either.map(elemType, (inner) => list(type.name, undefined, undefined, inner));
}
