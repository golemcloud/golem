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
import * as Option from "../../../newTypes/option";
import { TypeMappingScope } from './scope';

type TsType = CoreType.Type;

export type Ctx = {
  type: TsType;
  scope: Option.Option<TypeMappingScope>;
  scopeName?: string;
  parameterInScope: Option.Option<string>;
};

export function ctx(type: TsType, scope: Option.Option<TypeMappingScope>): Ctx {
  return {
    type,
    scope,
    scopeName: Option.isSome(scope) ? scope.val.name : undefined,
    parameterInScope: Option.isSome(scope)
      ? TypeMappingScope.paramName(scope.val)
      : Option.none(),
  };
}