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

let variantNameGlobalIdx = 0;

export function generateVariantCaseName(originalUnionName: string | undefined, termIdx: number): string {

  // if the union by itself does not have a name, generate a generic one using `case` prefix
  if (!originalUnionName) {
    variantNameGlobalIdx += 1;
    return `case${variantNameGlobalIdx}`
  }

  // otherwise, convert the original union name to kebab-case and append the term index
  // Example: type MyUnion = A | B | C
  // generates: my-union0, my-union1, my-union2
  const kebabCasedVariantName = convertVariantTypeNameToKebab(originalUnionName);
  return `${kebabCasedVariantName}${termIdx}`
}
