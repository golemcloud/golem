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

import { WitValue } from 'golem:rpc/types@0.2.2';
import * as Either from '../../../newTypes/either';
import * as Value from './Value';
import {
  serializeBinaryReferenceTsValue,
  serializeDefaultTsValue,
  serializeTextReferenceTsValue,
} from './serializer';
import { deserialize } from './deserializer';
import { AnalysedType } from '../types/analysedType';

export { WitValue } from 'golem:rpc/types@0.2.2';

export const fromTsValueDefault = (
  tsValue: any,
  analysedType: AnalysedType,
): Either.Either<WitValue, string> => {
  const valueEither = serializeDefaultTsValue(tsValue, analysedType);
  return Either.map(valueEither, Value.toWitValue);
};

// For RPC calls, we need wit-value representation of the binary reference (and not DataValue)
export const fromTsValueTextReference = (tsValue: any): WitValue => {
  const value = serializeTextReferenceTsValue(tsValue);

  return Value.toWitValue(value);
};

// For RPC calls, we need wit-value representation of the binary reference (and not DataValue)
export const fromTsValueBinaryReference = (tsValue: any): WitValue => {
  const value = serializeBinaryReferenceTsValue(tsValue);

  return Value.toWitValue(value);
};

export const toTsValue = (witValue: WitValue, expectedType: AnalysedType): any => {
  const value: Value.Value = Value.fromWitValue(witValue);
  return deserialize(value, expectedType);
};
