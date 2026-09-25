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
import { field, t } from '../src/internal/schema-model';
import { matchesSchemaType } from '../src/schema/union';

describe('plain-union structural matching', () => {
  it('requires prototype-named record fields to be own properties', () => {
    const constructorRecord = t.record([field('constructor', t.string())]);
    const toStringRecord = t.record([field('toString', t.string())]);

    expect(matchesSchemaType(new Map(), constructorRecord, {})).toBe(false);
    expect(matchesSchemaType(new Map(), toStringRecord, {})).toBe(false);
    expect(matchesSchemaType(new Map(), constructorRecord, { constructor: 'own' })).toBe(true);
    expect(matchesSchemaType(new Map(), toStringRecord, { toString: 'own' })).toBe(true);
  });
});
