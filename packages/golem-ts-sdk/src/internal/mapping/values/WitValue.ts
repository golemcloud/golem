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
import { WitValue } from 'golem:rpc/types@0.2.2';
import * as Either from '../../../newTypes/either';
import * as Value from './Value';

export { WitValue } from 'golem:rpc/types@0.2.2';

export const fromTsValue = (
  tsValue: any,
  tsType: Type.Type,
): Either.Either<WitValue, string> => {
  const valueEither = Value.fromTsValue(tsValue, tsType);
  return Either.map(valueEither, Value.toWitValue);
};

export const toTsValue = (witValue: WitValue, expectedType: Type.Type): any => {
  const value = Value.fromWitValue(witValue);
  return Value.toTsValue(value, expectedType);
};
