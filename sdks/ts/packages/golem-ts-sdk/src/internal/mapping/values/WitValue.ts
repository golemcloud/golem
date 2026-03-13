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

import { WitValue } from 'golem:core/types@1.5.0';
import * as Either from '../../../newTypes/either';
import {
  serializeBinaryReferenceToWitNodes,
  serializeTextReferenceToWitNodes,
  serializeToWitNodes,
} from './serializer';
import { deserializeNodes } from './deserializer';
import { AnalysedType } from '../types/analysedType';
import { WitNodeBuilder } from './WitNodeBuilder';

export { WitValue } from 'golem:core/types@1.5.0';

export const fromTsValueDefault = (
  tsValue: any,
  analysedType: AnalysedType,
): Either.Either<WitValue, string> => {
  const builder = new WitNodeBuilder();
  const result = serializeToWitNodes(tsValue, analysedType, builder);
  if (Either.isLeft(result)) return result;
  return Either.right(builder.build());
};

// For RPC calls, we need wit-value representation of the binary reference (and not DataValue)
export const fromTsValueTextReference = (tsValue: any): WitValue => {
  const builder = new WitNodeBuilder();
  serializeTextReferenceToWitNodes(tsValue, builder);
  return builder.build();
};

// For RPC calls, we need wit-value representation of the binary reference (and not DataValue)
export const fromTsValueBinaryReference = (tsValue: any): WitValue => {
  const builder = new WitNodeBuilder();
  serializeBinaryReferenceToWitNodes(tsValue, builder);
  return builder.build();
};

export const toTsValue = (witValue: WitValue, expectedType: AnalysedType): any => {
  return deserializeNodes(witValue.nodes, 0, expectedType);
};
