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
import { Ctx } from './ctx';
import { AnalysedType } from './analysedType';
import { TypeMapper } from './typeMapper';

type TsType = CoreType.Type;

// Types that are known but tagged as "others"
type OthersCtx = Ctx & { type: Extract<TsType, { kind: "others" }> };

export function handleOthers({ type }: OthersCtx, _mapper: TypeMapper): Either.Either<AnalysedType, string> {
  const customTypeName = type.name

  if (!customTypeName) {
    return Either.left("Unsupported type (anonymous) found.");
  }

  if (customTypeName === 'any') {
    return Either.left("Unsupported type `any`. Use a specific type instead");
  }

  if (customTypeName === 'Date') {
    return Either.left("Unsupported type `Date`. Use a string in ISO 8601 format instead");
  }

  if (customTypeName === 'next') {
    return Either.left("Unsupported type `Iterator`. Use `Array` type instead");
  }

  if (customTypeName.includes('asyncIterator')) {
    return Either.left(`Unsupported type \`AsyncIterator\`. Use \`Array\` type instead`);
  }

  if (customTypeName.includes('iterator')) {
    return Either.left(`Unsupported type \`Iterable\`. Use \`Array\` type instead`);
  }

  if (customTypeName.includes('asyncIterable')) {
    return Either.left(`Unsupported type \`AsyncIterable\`. Use \`Array\` type instead`);
  }

  if (customTypeName === 'Record') {
    return Either.left(`Unsupported type \`${customTypeName}\`. Use a plain object or a \`Map\` type instead`);
  }

  if (type.recursive) {
    return Either.left(`\`${customTypeName}\` is recursive.\nRecursive types are not supported yet. \nHelp: Avoid recursion in this type (e.g. using index-based node lists) and try again.`);
  } else {
    return Either.left(`Unsupported type \`${customTypeName}\``);
  }
}
