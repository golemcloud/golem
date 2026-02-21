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
import { AnalysedType } from './analysedType';
import { TypeMapper } from './typeMapper';

type TsType = CoreType.Type;

// Types that are known but tagged as "others"
type OthersCtx = Ctx & { type: Extract<TsType, { kind: 'others' }> };

export function handleOthers(
  { type }: OthersCtx,
  _mapper: TypeMapper,
): Either.Either<AnalysedType, string> {
  const name = type.name;

  if (!name) {
    return Either.left('Unsupported type (anonymous) found.');
  }

  for (const rule of REJECT_RULES) {
    if (rule.test(name)) {
      return Either.left(rule.message(name));
    }
  }

  if (type.recursive) {
    return Either.left(
      `\`${name}\` is recursive.\n` +
        `Recursive types are not supported yet.\n` +
        `Help: Avoid recursion in this type (e.g. using index-based node lists) and try again.`,
    );
  }

  return Either.left(`Unsupported type \`${name}\``);
}

type RejectRule = {
  test: (name: string) => boolean;
  message: (name: string) => string;
};

const REJECT_RULES: RejectRule[] = [
  {
    test: (name) => name === 'any',
    message: () => 'Unsupported type `any`. Use a specific type instead',
  },
  {
    test: (name) => name === 'Date',
    message: () => 'Unsupported type `Date`. Use a string in ISO 8601 format instead',
  },
  {
    test: (name) => name === 'next',
    message: () => 'Unsupported type `Iterator`. Use `Array` type instead',
  },
  {
    test: (name) => name.includes('asyncIterator'),
    message: () => 'Unsupported type `AsyncIterator`. Use `Array` type instead',
  },
  {
    test: (name) => name.includes('iterator'),
    message: () => 'Unsupported type `Iterable`. Use `Array` type instead',
  },
  {
    test: (name) => name.includes('asyncIterable'),
    message: () => 'Unsupported type `AsyncIterable`. Use `Array` type instead',
  },
  {
    test: (name) => name === 'Record',
    message: (name) => `Unsupported type \`${name}\`. Use a plain object or a \`Map\` type instead`,
  },
];
