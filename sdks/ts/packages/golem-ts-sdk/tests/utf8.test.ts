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
import { decodeUtf8 } from '../src/internal/utf8';

const encoder = new TextEncoder();

describe('strict UTF-8 decoding', () => {
  it('reuses the decoder across repeated complete payloads', () => {
    expect(decodeUtf8(encoder.encode('first'))).toBe('first');
    expect(decodeUtf8(encoder.encode('árvíztűrő tükörfúrógép'))).toBe('árvíztűrő tükörfúrógép');
    expect(decodeUtf8(encoder.encode('third'))).toBe('third');
  });

  it('rejects malformed UTF-8 and remains usable afterward', () => {
    expect(() => decodeUtf8(Uint8Array.from([0xc3, 0x28]))).toThrow();
    expect(decodeUtf8(encoder.encode('valid afterward'))).toBe('valid afterward');
  });
});
