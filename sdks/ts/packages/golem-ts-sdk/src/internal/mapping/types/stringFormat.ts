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

// Cache for combining mark detection (U+0300-U+036F range covers most diacritics)
const COMBINING_MARK_START = 0x0300;
const COMBINING_MARK_END = 0x036f;

function isCombiningMarkFast(code: number): boolean {
  // Fast path for common combining marks (U+0300-U+036F)
  if (code >= COMBINING_MARK_START && code <= COMBINING_MARK_END) return true;
  // Fallback for other mark ranges (rare)
  if (code >= 0x1ab0 && code <= 0x1aff) return true; // Extended combining
  if (code >= 0x1dc0 && code <= 0x1dff) return true; // Phonetic extensions
  if (code >= 0x20d0 && code <= 0x20ff) return true; // Combining diacritical marks for symbols
  return false;
}

function getCaseType(c: string): WordMode {
  const lower = c.toLowerCase();
  const upper = c.toUpperCase();
  if (lower === upper) return 'Boundary'; // No case change, likely non-letter
  return c === lower ? 'Lowercase' : 'Uppercase';
}

/**
 * Converts a string to kebab-case.
 *
 * Ported from the Rust heck crate https://github.com/openkylin/rust-heck
 */
function toKebab(str: string): string {
  const result: string[] = [];
  let firstWord = true;

  // Split by non-letter/non-number characters (but keep combining marks with letters)
  const words = splitWords(str);

  for (const word of words) {
    let init = 0;
    let mode: WordMode = 'Boundary';
    let i = 0;
    const wordLen = word.length;

    while (i < wordLen) {
      const c = word[i];

      // Skip underscores within words
      if (c === '_') {
        if (init === i) {
          init++;
        }
        i++;
        continue;
      }

      // Look ahead for combining marks
      let nextCharIndex = i + 1;
      while (nextCharIndex < wordLen && isCombiningMarkFast(word.charCodeAt(nextCharIndex))) {
        nextCharIndex++;
      }

      const nextChar = nextCharIndex < wordLen ? word[nextCharIndex] : undefined;
      const nextMode = nextChar !== undefined ? getCaseType(nextChar) : 'Boundary';
      const curMode = getCaseType(c);

      if (nextChar !== undefined) {
        // Word boundary if next is underscore
        if (nextChar === '_') {
          if (!firstWord) result.push('-');
          result.push(transformWord(word.slice(init, nextCharIndex)));
          firstWord = false;
          init = nextCharIndex;
          mode = 'Boundary';
          i = nextCharIndex;
        }
        // Word boundary if current is lowercase and next is uppercase
        else if (curMode === 'Lowercase' && nextMode === 'Uppercase') {
          if (!firstWord) result.push('-');
          result.push(transformWord(word.slice(init, nextCharIndex)));
          firstWord = false;
          init = nextCharIndex;
          mode = 'Boundary';
          i = nextCharIndex;
        }
        // Word boundary if current and previous are uppercase and next is lowercase
        else if (mode === 'Uppercase' && curMode === 'Uppercase' && nextMode === 'Lowercase') {
          if (firstWord) {
            firstWord = false;
          } else {
            result.push('-');
          }
          result.push(transformWord(word.slice(init, i)));
          init = i;
          mode = 'Boundary';
          i++;
        } else {
          // No boundary, just update mode
          mode = curMode;
          i++;
        }
      } else {
        // Last character(s) of word
        if (firstWord) {
          firstWord = false;
        } else {
          result.push('-');
        }
        result.push(transformWord(word.slice(init)));
        break;
      }
    }
  }

  return result.join('');
}

function splitWords(str: string): string[] {
  // Split on non-letter/non-number but preserve combining marks
  const pattern = /[\p{L}\p{N}_\p{M}]+/gu;
  const matches = str.match(pattern);
  return matches || [];
}

function transformWord(word: string): string {
  // Apply lowercase transformation with special handling for Greek sigma
  let result = '';
  const len = word.length;
  for (let i = 0; i < len; i++) {
    const c = word[i];
    if (c === 'Σ' && i === len - 1) {
      result += 'ς';
    } else {
      result += c.toLowerCase();
    }
  }
  return result;
}

export function convertTypeNameToKebab(typeName: string): string {
  return toKebab(typeName);
}

export function convertOptionalTypeNameToKebab(typeName: string | undefined): string | undefined {
  return typeName ? convertTypeNameToKebab(typeName) : undefined;
}

export function convertAgentMethodNameToKebab(methodName: string): string {
  return toKebab(methodName);
}

export function convertVariantTypeNameToKebab(typeName: string): string {
  return toKebab(typeName);
}
