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
import { Ctx } from './ctx';
import { AnalysedType } from './analysedType';
import { fromTsTypeInternal } from './typeMapping';

type TsType = CoreType.Type;

type PromiseCtx = Ctx & { type: Extract<TsType, { kind: "promise" }> };

export function handlePromise({ type }: PromiseCtx): Either.Either<AnalysedType, string> {
  const inner = type.element;
  return fromTsTypeInternal(inner, Option.none());
}