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

// Represents the scope of the type that's being mapped.
// Example:
// For the following code:
//
// ```ts
// interface MyInterface { a?: MyData }
// ```
//
// During type mapping of `MyInterface`, the mapping of `MyData` will come into picture,
// and at this point, the scope of mapping is as follows:
//
// ```ts
//   { scope: 'interface', name: 'MyData', parameterName: 'a', hasQuestionMark: true  }
// ```
export type TypeScope =
  | {
      scope: 'interface' | 'object' | 'method' | 'constructor';
      name: string;
      parameterName: string;
      hasQuestionMark: boolean;
    }
  | {
      scope: 'others';
      name: string;
    };

export const TypeScope = {
  isQuestionMarkOptional(scope: TypeScope) {
    return (
      (scope.scope === 'interface' ||
        scope.scope === 'object' ||
        scope.scope === 'method' ||
        scope.scope === 'constructor') &&
      scope.hasQuestionMark
    );
  },

  paramName(scope: TypeScope): string | undefined {
    if (
      scope.scope === 'interface' ||
      scope.scope === 'object' ||
      scope.scope === 'method' ||
      scope.scope === 'constructor'
    ) {
      return scope.parameterName;
    }

    return undefined;
  },

  interface(name: string, parameterName: string, hasQuestionMark: boolean): TypeScope {
    return {
      scope: 'interface',
      name,
      parameterName,
      hasQuestionMark: hasQuestionMark,
    };
  },

  object(name: string, parameterName: string, hasQuestionMark: boolean): TypeScope {
    return {
      scope: 'object',
      name,
      parameterName,
      hasQuestionMark: hasQuestionMark,
    };
  },

  method(name: string, parameterName: string, hasQuestionMark: boolean): TypeScope {
    return {
      scope: 'method',
      name,
      parameterName,
      hasQuestionMark: hasQuestionMark,
    };
  },

  constructor(name: string, parameterName: string, hasQuestionMark: boolean): TypeScope {
    return {
      scope: 'constructor',
      name: name,
      parameterName: parameterName,
      hasQuestionMark: hasQuestionMark,
    };
  },

  others(name: string): TypeScope {
    return {
      scope: 'others',
      name,
    };
  },
};
