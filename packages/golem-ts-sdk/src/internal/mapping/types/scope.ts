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

import * as Option from '../../../newTypes/option';

export type TypeMappingScope = {
  scope: 'interface' | 'object' | 'method' | 'constructor';
  name: string;
  parameterName: string
  optional: boolean
}
  | {
  scope: 'others';
  name: string;
};

export const TypeMappingScope = {
  isOptionalParam(scope: TypeMappingScope) {
    return (scope.scope === 'interface' ||
      scope.scope === 'object' ||
      scope.scope === 'method' || scope.scope === 'constructor') && scope.optional;
  },

  paramName(scope: TypeMappingScope): Option.Option<string> {
    if (scope.scope === 'interface' ||
      scope.scope === 'object' ||
      scope.scope === 'method' || scope.scope === 'constructor') {
      return Option.some(scope.parameterName);
    }

    return Option.none()

  },

  interface(name: string, parameterName: string, optional: boolean): TypeMappingScope {
    return {
      scope: 'interface',
      name,
      parameterName,
      optional,
    };
  },

  object(name: string, parameterName: string, optional: boolean): TypeMappingScope {
    return {
      scope: 'object',
      name,
      parameterName,
      optional,
    };
  },

  method(name: string, parameterName: string, optional: boolean): TypeMappingScope {
    return {
      scope: 'method',
      name,
      parameterName,
      optional,
    };
  },

  constructor(name: string, parameterName: string, optional: boolean): TypeMappingScope {
    return {
      scope: 'constructor',
      name: name,
      parameterName: parameterName,
      optional: optional,
    }

  },

  others(name: string): TypeMappingScope {
    return {
      scope: 'others',
      name,
    };
  }
}