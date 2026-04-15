// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
import {
  AnalysedType,
  bool,
  case_,
  f64,
  field,
  option,
  record,
  s64,
  str,
  u32,
  u64,
  unitCase,
  variant,
} from './analysedType';
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
  config: unsupported('Config'),
  principal: handlePrincipal,
  'quota-token': handleQuotaToken,
};

function handleQuotaToken(): Either.Either<AnalysedType, string> {
  const envIdType = record('EnvironmentId', [
    field('uuid', record('Uuid', [field('highBits', u64(true)), field('lowBits', u64(true))])),
  ]);
  const datetimeType = record('Datetime', [
    field('seconds', u64(true)),
    field('nanoseconds', u32()),
  ]);
  const analysedType = record('QuotaTokenRecord', [
    field('environmentId', envIdType),
    field('resourceName', str()),
    field('expectedUse', u64(true)),
    field('lastCredit', s64(true)),
    field('lastCreditAt', datetimeType),
  ]);
  return Either.right(analysedType);
}

function handlePrincipal(): Either.Either<AnalysedType, string> {
  const uuidType = record('Uuid', [field('highBits', u64(true)), field('lowBits', u64(true))]);

  const accountIdType = record('AccountId', [field('uuid', uuidType)]);

  const componentIdType = record('ComponentId', [field('uuid', uuidType)]);

  const agentIdType = record('AgentId', [
    field('componentId', componentIdType),
    field('agentId', str()),
  ]);

  const analysedType = variant(
    'Principal',
    [],
    [
      case_(
        'oidc',
        record('OidcPrincipal', [
          field('sub', str()),
          field('issuer', str()),
          field('email', option(undefined, 'undefined', str())),
          field('name', option(undefined, 'undefined', str())),
          field('emailVerified', option(undefined, 'undefined', bool())),
          field('givenName', option(undefined, 'undefined', str())),
          field('familyName', option(undefined, 'undefined', str())),
          field('picture', option(undefined, 'undefined', str())),
          field('preferredUsername', option(undefined, 'undefined', str())),
          field('claims', str()),
        ]),
      ),
      case_('agent', record('AgentPrincipal', [field('agentId', agentIdType)])),
      case_('golem-user', record('GolemUserPrincipal', [field('accountId', accountIdType)])),
      unitCase('anonymous'),
    ],
  );
  return Either.right(analysedType);
}

function unsupported<K extends TsType['kind']>(kind: string): Handler<K> {
  return ({ scopeName, parameterInScope }) =>
    Either.left(
      `Unsupported type \`${kind}\`` +
        (scopeName ? ` in ${scopeName}` : '') +
        (parameterInScope ? ` for parameter \`${parameterInScope}\`` : ''),
    );
}

function unsupportedWithHint<K extends TsType['kind']>(kind: string, hint: string): Handler<K> {
  return ({ scopeName, parameterInScope }) =>
    Either.left(
      `Unsupported type \`${kind}\`${scopeName ? ` in ${scopeName}` : ''}` +
        (parameterInScope ? ` for parameter \`${parameterInScope}\`` : '') +
        `. Hint: ${hint}`,
    );
}
