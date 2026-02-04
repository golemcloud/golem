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
import { TypeScope } from './scope';

type TsType = CoreType.Type;

// The `handler` deals with `Ctx` rather than `TypeMappingScope`.
// A scope is a pre-calculated details along with the scope details
export type Ctx = {
  type: TsType;
  scope: TypeScope | undefined;
  scopeName?: string;
  parameterInScope: string | undefined;
};

export function createCtx(type: TsType, scope: TypeScope | undefined): Ctx {
  return {
    type,
    scope,
    scopeName: scope?.name,
    parameterInScope: scope ? TypeScope.paramName(scope) : undefined,
  };
}
