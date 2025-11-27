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

type WordMode = 'Boundary' | 'Lowercase' | 'Uppercase';

function toKebab(str: string): string {
  let result = '';
  let firstWord = true;

  // Split by non-letter/non-number characters (but keep combining marks with letters)
  // This uses a more sophisticated pattern that handles combining diacritics
  const words = splitWords(str);

  for (const word of words) {
    // Process each word for internal case transitions
    let init = 0;
    let mode: WordMode = 'Boundary';

    // Work with code points to handle combining marks properly
    let i = 0;
    while (i < word.length) {
      const c = word[i];

      // Skip underscores within words and adjust init
      if (c === '_') {
        if (init === i) {
          init++;
        }
        i++;
        continue;
      }

      // Look ahead for combining marks
      let nextCharIndex = i + 1;
      while (nextCharIndex < word.length && isCombiningMark(word[nextCharIndex])) {
        nextCharIndex++;
      }

      const nextChar = nextCharIndex < word.length ? word[nextCharIndex] : undefined;

      // Determine next mode based on current character case
      let nextMode: WordMode = 'Boundary';
      if (isLowerCaseChar(c)) {
        nextMode = 'Lowercase';
      } else if (isUpperCaseChar(c)) {
        nextMode = 'Uppercase';
      }

      if (nextChar !== undefined) {
        // Word boundary if next is underscore
        if (nextChar === '_') {
          if (!firstWord) {
            result += '-';
          }
          result += transformWord(word.slice(init, nextCharIndex));
          firstWord = false;
          init = nextCharIndex;
          mode = 'Boundary';
          i = nextCharIndex;
        }
        // Word boundary if current is lowercase and next is uppercase
        else if (
          nextMode === 'Lowercase' &&
          isUpperCaseChar(nextChar)
        ) {
          if (!firstWord) {
            result += '-';
          }
          result += transformWord(word.slice(init, nextCharIndex));
          firstWord = false;
          init = nextCharIndex;
          mode = 'Boundary';
          i = nextCharIndex;
        }
        // Word boundary if current and previous are uppercase and next is lowercase
        else if (
          mode === 'Uppercase' &&
          isUpperCaseChar(c) &&
          isLowerCaseChar(nextChar)
        ) {
          if (!firstWord) {
            result += '-';
          } else {
            firstWord = false;
          }
          result += transformWord(word.slice(init, i));
          init = i;
          mode = 'Boundary';
          i++;
        } else {
          // No boundary, just update mode
          mode = nextMode;
          i++;
        }
      } else {
        // Last character(s) of word
        if (!firstWord) {
          result += '-';
        } else {
          firstWord = false;
        }
        result += transformWord(word.slice(init));
        break;
      }
    }
  }

  return result;
}

function splitWords(str: string): string[] {
  // Split on non-letter/non-number but preserve combining marks
  // Match: letters, numbers, underscores, combining marks, and their combinations
  const pattern = /[\p{L}\p{N}_\p{M}]+/gu;
  const matches = str.match(pattern);
  return matches || [];
}

function isCombiningMark(c: string): boolean {
  // Check if character is a combining mark (diacritical mark)
  const code = c.charCodeAt(0);
  // Combining diacritical marks range and other mark categories
  return /\p{M}/u.test(c);
}

function isLowerCaseChar(c: string): boolean {
  // Check if it's a letter (has case) and is lowercase
  const lower = c.toLowerCase();
  const upper = c.toUpperCase();
  return lower !== upper && c === lower;
}

function isUpperCaseChar(c: string): boolean {
  // Check if it's a letter (has case) and is uppercase
  const lower = c.toLowerCase();
  const upper = c.toUpperCase();
  return lower !== upper && c === upper;
}

function transformWord(word: string): string {
  // Apply lowercase transformation with special handling for Greek sigma
  return Array.from(word)
    .map((c, i, arr) => {
      if (c === 'Σ' && i === arr.length - 1) {
        return 'ς';
      }
      return c.toLowerCase();
    })
    .join('');
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

