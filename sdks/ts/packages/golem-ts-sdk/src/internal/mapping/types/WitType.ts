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
import { WitTypeBuilder } from './witTypeBuilder';
import * as Either from '../../../newTypes/either';
import { WitType } from 'golem:agent/common@1.5.0';
import { TypeScope } from './scope';
import { AnalysedType } from './analysedType';
import { typeMapper } from './typeMapperImpl';

export { WitType } from 'golem:core/types@1.5.0';

/**
 * Creates a WitType from a TypeScript Type
 * Usage:
 *
 * ```ts
 *   import * as WitType from './WitType';
 *
 *   WitType.fromTsType(type, scope)
 * ```
 */
export const fromTsType = (
  type: Type.Type,
  scope: TypeScope | undefined,
): Either.Either<[WitType, AnalysedType], string> => {
  const analysedTypeEither = typeMapper(type, scope);
  return Either.flatMap(analysedTypeEither, (analysedType) => {
    const witType = fromAnalysedType(analysedType);
    return Either.right([witType, analysedType]);
  });
};

/**
 * Creates a WitType from an AnalysedType
 *
 * Usage:
 *
 * ```ts
 *   import * as WitType from './WitType';
 *   WitType.fromAnalysedType(analysedType);
 * ```
 */
export const fromAnalysedType = (analysedType: AnalysedType): WitType => {
  const builder = new WitTypeBuilder();
  builder.add(analysedType);
  return builder.build();
};
