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

const KEBAB_CHECK_REGEX = /^[a-z]+(-[a-z]+)*$/;

export function isKebabCase(str: string): boolean {
  return KEBAB_CHECK_REGEX.test(str);
}

const TO_KEBAB_SOURCE_REGEX = /([a-z])([A-Z])/g;
const TO_KEBAB_TARGET_REGEX = /[\s_]+/g;

function toKebab(str: string): string {
  return str
    .replace(TO_KEBAB_SOURCE_REGEX, '$1-$2')
    .replace(TO_KEBAB_TARGET_REGEX, '-')
    .toLowerCase();
}

export function convertTypeNameToKebab(typeName: string): string {
  return toKebab(typeName);
}

export function convertOptionalTypeNameToKebab(typeName: string | undefined): string | undefined {
  return typeName ? convertTypeNameToKebab(typeName) : undefined;
}

export function convertAgentMethodNameToKebab(methodName: string): string {
  return toKebab(methodName)
}

export function convertVariantTypeNameToKebab(typeName: string): string{
  return toKebab(typeName)
}

