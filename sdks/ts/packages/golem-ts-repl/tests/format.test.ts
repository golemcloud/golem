// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

import { describe, expect, it } from 'vitest';
import { wrapSnippetInfoLine } from '../src/format';

describe('wrapSnippetInfoLine', () => {
  it('does not wrap short callable signatures', () => {
    const line = '(x: number): Promise<number>;';

    expect(wrapSnippetInfoLine(line, 120)).toEqual([line]);
  });

  it('wraps long callable signatures by top-level parameters', () => {
    expect(
      wrapSnippetInfoLine(
        '(x: number, y: string, z: { tag: "case1"; val: number; } | { tag: "case2"; val: string; }): Promise<number>;',
        50,
      ),
    ).toEqual([
      '(',
      '  x: number,',
      '  y: string,',
      '  z: { tag: "case1"; val: number; }',
      '    | { tag: "case2"; val: string; }',
      '): Promise<number>;',
    ]);
  });

  it('wraps long property callable signatures', () => {
    expect(
      wrapSnippetInfoLine('schedule: (scheduleAt: string, x: number, y: string) => void;', 40),
    ).toEqual([
      'schedule: (',
      '  scheduleAt: string,',
      '  x: number,',
      '  y: string',
      ') => void;',
    ]);
  });

  it('does not split generic type arguments', () => {
    expect(
      wrapSnippetInfoLine(
        '(x: Map<string, number>, y: Array<{ a: string, b: number }>): void;',
        40,
      ),
    ).toEqual([
      '(',
      '  x: Map<string, number>,',
      '  y: Array<{ a: string, b: number }>',
      '): void;',
    ]);
  });

  it('preserves base indentation when wrapping', () => {
    expect(
      wrapSnippetInfoLine(
        '  abortable: (signal: AbortSignal, x: number, y: string) => Promise<number>;',
        48,
      ),
    ).toEqual([
      '  abortable: (',
      '    signal: AbortSignal,',
      '    x: number,',
      '    y: string',
      '  ) => Promise<number>;',
    ]);
  });
});
