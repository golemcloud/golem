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

import { Type } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../newTypes/either';
import { RuntimeOutput } from '../typeInfoInternal';
import { resolveParamType } from './helpers';

/**
 * Resolve a method's return type into its schema-native {@link RuntimeOutput}:
 * `unit` for `void` / `undefined` / `null` (incl. wrapped in a `Promise`), and
 * `single` otherwise. `Result<void, E>` / `Result<T, void>` stay `single` (the
 * value mapper encodes the absent arm).
 */
export function resolveMethodOutput(returnType: Type.Type): Either.Either<RuntimeOutput, string> {
  const inner = returnType.kind === 'promise' ? returnType.element : returnType;

  if (inner.kind === 'void' || inner.kind === 'undefined' || inner.kind === 'null') {
    return Either.right({ tag: 'unit' });
  }

  return Either.flatMap(resolveParamType(undefined, inner), (type) => {
    if (type.tag === 'principal' || type.tag === 'config') {
      return Either.left(`A method cannot return a \`${type.tag}\` value`);
    }
    return Either.right({ tag: 'single', type });
  });
}
