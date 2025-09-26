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

import {Type} from "@golemcloud/golem-ts-types-core";
import {WitTypeBuilder} from "./witTypeBuilder";
import * as Either from "../../../newTypes/either";
import * as Option from "../../../newTypes/option";
import {WitType} from "golem:agent/common";
import * as AnalysedType from "./AnalysedType";
import { TypeMappingScope } from './scope';

export { WitType } from "golem:rpc/types@0.2.2";

export const fromTsType = (type: Type.Type, scope: Option.Option<TypeMappingScope>): Either.Either<[WitType, AnalysedType.AnalysedType], string> => {
    const analysedTypeEither = AnalysedType.fromTsType(type, scope);
    return Either.flatMap(analysedTypeEither, (analysedType) => {
      const witType = fromAnalysedType(analysedType);
      return Either.right(([witType, analysedType]));
    });
};


export const fromAnalysedType = (analysedType: AnalysedType.AnalysedType): WitType => {
    const builder = new WitTypeBuilder();
    builder.add(analysedType);
    return builder.build();
}