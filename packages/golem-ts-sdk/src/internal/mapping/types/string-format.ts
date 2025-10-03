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

export function isNumberString(name: string): boolean {
  return !isNaN(Number(name));
}

export function trimQuotes(s: string): string {
  if (s.startsWith('"') && s.endsWith('"')) {
    return s.slice(1, -1);
  }
  return s;
}


export function isKebabCase(str: string): boolean {
  return /^[a-z]+(-[a-z]+)*$/.test(str);
}

export function convertTypeNameToKebab(typeName: string): string {
  return typeName
  .replace(/([a-z])([A-Z])/g, '$1-$2')
  .replace(/[\s_]+/g, '-')
  .toLowerCase();
}

export function convertOptionalTypeNameToKebab(typeName: string | undefined): string | undefined {
  return typeName ? convertTypeNameToKebab(typeName) : undefined;
}
