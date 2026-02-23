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
import { AnalysedType, bool, f64, str, u64 } from './analysedType';
import { Ctx } from './ctx';
import { handleUnion } from './union';
import { handleTuple } from './tuple';
import { handleObject } from './object';
import { handleInterface } from './interface';
import { handlePromise } from './promise';
import { handleMap } from './map';
import { handleLiteral } from './literal';
import { handleAlias } from './alias';
import { handleOthers } from './others';
import { handleUnresolved } from './unresolved';
import { handleArray } from './array';
import { TypeMapper } from './typeMapper';

type TsType = CoreType.Type;

export function callHandler<K extends TsType['kind']>(
  kind: K,
  ctx: Ctx,
  typeMapper: TypeMapper,
): Either.Either<AnalysedType, string> {
  const handler = handlers[kind] as Handler<K>;
  return handler(ctx as Ctx & { type: Extract<TsType, { kind: K }> }, typeMapper);
}

export type Handler<K extends TsType['kind']> = (
  ctx: Ctx & { type: Extract<TsType, { kind: K }> },
  mapper: TypeMapper,
) => Either.Either<AnalysedType, string>;

const handlers: { [K in TsType['kind']]: Handler<K> } = {
  boolean: () => Either.right(bool()),
  number: () => Either.right(f64()),
  string: () => Either.right(str()),
  bigint: () => Either.right(u64(true)),

  null: unsupported('null'),
  undefined: unsupported('undefined'),
  void: unsupported('void'),

  tuple: handleTuple,
  union: handleUnion,
  object: handleObject,
  interface: handleInterface,
  class: unsupportedWithHint('class', 'Use object instead.'),
  promise: handlePromise,
  map: handleMap,
  literal: handleLiteral,
  alias: handleAlias,
  others: handleOthers,
  'unresolved-type': handleUnresolved,
  array: handleArray,
  principal: unsupportedWithHint('Principal', '')
};

function unsupported(kind: string): Handler<any> {
  return ({ scopeName, parameterInScope }) =>
    Either.left(
      `Unsupported type \`${kind}\`` +
        (scopeName ? ` in ${scopeName}` : '') +
        (parameterInScope ? ` for parameter \`${parameterInScope}\`` : ''),
    );
}

function unsupportedWithHint(kind: string, hint: string): Handler<any> {
  return ({ scopeName, parameterInScope }) =>
    Either.left(
      `Unsupported type \`${kind}\`${scopeName ? ` in ${scopeName}` : ''}` +
        (parameterInScope ? ` for parameter \`${parameterInScope}\`` : '') +
        `. Hint: ${hint}`,
    );
}
