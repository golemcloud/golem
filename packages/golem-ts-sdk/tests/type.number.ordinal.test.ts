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

import { test, expect } from 'vitest';
import fc from 'fast-check';
import { numberToOrdinalKebab } from '../src/internal/mapping/types/typeIndexOrdinal';

test('numberToOrdinalKebab produces valid kebab-case ordinals', () => {
  fc.assert(
    fc.property(fc.integer({ min: 1, max: 999 }), (n) => {
      const result = numberToOrdinalKebab(n);

      expect(result).toMatch(/^[a-z-]+$/);

      expect(result.includes(' ')).toBe(false);

      expect(
        result.endsWith('th') ||
          result.endsWith('st') ||
          result.endsWith('nd') ||
          result.endsWith('rd'),
      ).toBe(true);

      expect(numberToOrdinalKebab(n)).toBe(result);
    }),
  );
});

// A specific test to really ensure the 20 members of the union are correct
test('numberToOrdinalKebab returns correct ordinals for first 20', () => {
  const expected: Record<number, string> = {
    1: 'first',
    2: 'second',
    3: 'third',
    4: 'fourth',
    5: 'fifth',
    6: 'sixth',
    7: 'seventh',
    8: 'eighth',
    9: 'ninth',
    10: 'tenth',
    11: 'eleventh',
    12: 'twelfth',
    13: 'thirteenth',
    14: 'fourteenth',
    15: 'fifteenth',
    16: 'sixteenth',
    17: 'seventeenth',
    18: 'eighteenth',
    19: 'nineteenth',
    20: 'twentieth',
  };

  for (const [num, word] of Object.entries(expected)) {
    expect(numberToOrdinalKebab(Number(num))).toBe(word);
  }
});
