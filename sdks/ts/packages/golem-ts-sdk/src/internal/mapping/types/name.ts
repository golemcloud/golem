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

import { convertVariantTypeNameToKebab } from './stringFormat';

let GlobalVariantTermIdx = 0;

// A union is converted to a variant in WIT.
// If they are not tagged unions, then it implies of the terms of the union are anonymous
// If the union by itself is also anonymous (ex: function foo(x: string | number), then
// we need to generate the names for the variant terms. The name will start with `case`
// suffixed with a globally incremented `GlobalVariantTermIdx`.
// That is, `string | number` is converted to `variant { case0(string), case1(number) }`.
// Note that `union` handler keeps a cache of these which helps with reusing the same variant whenever
// `string | number` appears in the code
//
// If the union has a name (Ex: type MyUnion = string | number), then we simply suffix the actual name of the union
// with the actual index (termIdx argument below) of the variant
// That is, `MyUnion` is converted to `variant { my-union-0(string), my-union-1(number)`
export function generateVariantTermName(
  originalUnionName: string | undefined,
  termIdx: number,
): string {
  // if the union by itself does not have a name, generate a generic one using `case` prefix
  if (!originalUnionName) {
    GlobalVariantTermIdx += 1;
    return `case${GlobalVariantTermIdx}`;
  }

  // otherwise, convert the original union name to kebab-case and append the term index
  // Example: type MyUnion = A | B | C
  // generates: my-union0, my-union1, my-union2
  const kebabCasedVariantName = convertVariantTypeNameToKebab(originalUnionName);
  return `${kebabCasedVariantName}${termIdx}`;
}
