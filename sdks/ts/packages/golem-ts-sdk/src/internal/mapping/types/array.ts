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

import {  Type as CoreType } from '@golemcloud/golem-ts-types-core';
import * as Either from "../../../newTypes/either";
import * as Option from "../../../newTypes/option";
import { Ctx } from './ctx';
import { AnalysedType, f32, f64, list, s16, s32, s64, s8, u16, u32, u64, u8 } from './analysedType';
import { fromTsTypeInternal } from './typeMapping';

type TsType = CoreType.Type;
type ArrayCtx = Ctx & { type: Extract<TsType, { kind: "array" }> };

export function handleArray({ type }: ArrayCtx): Either.Either<AnalysedType, string> {
  const name = type.name;

  switch (name) {
    case "Float64Array": return Either.right(list(undefined, 'f64', undefined, f64()));
    case "Float32Array": return Either.right(list(undefined, 'f32', undefined, f32()));
    case "Int8Array":    return Either.right(list(undefined, 'i8', undefined, s8()));
    case "Uint8Array":   return Either.right(list(undefined,  'u8', undefined, u8()));
    case "Int16Array":   return Either.right(list(undefined,  'i16', undefined, s16()));
    case "Uint16Array":  return Either.right(list(undefined,  'u16',  undefined, u16()));
    case "Int32Array":   return Either.right(list(undefined, 'i32', undefined, s32()));
    case "Uint32Array":  return Either.right(list(undefined, 'u32',  undefined, u32()));
    case "BigInt64Array":  return Either.right(list(undefined, 'big-i64', undefined, s64(true)));
    case "BigUint64Array": return Either.right(list(undefined,'big-u64', undefined, u64(true,)));
  }

  const arrayElementType =
    (type.kind === "array") ? type.element : undefined;

  if (!arrayElementType) {
    return Either.left("Unable to determine the array element type");
  }

  const elemType = fromTsTypeInternal(arrayElementType, Option.none());

  return Either.map(elemType, (inner) => list(type.name, undefined, undefined, inner));
}
