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
import { InvocationId } from '../src';
import { Uuid } from '../src/uuid';

const uuidText = '0123abcd-4567-89ef-fedc-ba9876543210';
const uuid = new Uuid(0x0123abcd456789efn, 0xfedcba9876543210n);

describe('InvocationId', () => {
  it('round-trips an agent ID that contains dots, slashes and quotes', () => {
    const agentId = `Agent("a.b/c",'d.e')`;
    const id = new InvocationId(agentId, uuid);
    const text = id.toString();
    expect(text).toBe(`${agentId}.${uuidText}`);

    const parsed = InvocationId.parse(text);
    expect(parsed.agentId).toBe(agentId);
    expect(parsed.uuid).toEqual(uuid);
    expect(parsed.toString()).toBe(text);
  });

  it('accepts an agent ID that ends with a dot', () => {
    const parsed = InvocationId.parse(`Agent()..${uuidText}`);
    expect(parsed.agentId).toBe('Agent().');
    expect(parsed.uuid).toEqual(uuid);
  });

  it.each([
    ['a missing dot', `Agent()${uuidText}`],
    ['a dot at the wrong position', `Agent().${uuidText}x`],
    ['a text shorter than a UUID', 'Agent().1234'],
    ['a bad UUID', 'Agent().0123abcd-4567-89ef-fedc-ba987654321g'],
    ['a UUID without hyphens', 'Agent().0123abcd456789effedcba9876543210xxxx'],
    ['an empty agent ID', `.${uuidText}`],
    ['an empty text', ''],
  ])('rejects %s', (_, text) => {
    expect(() => InvocationId.parse(text)).toThrow(/Invalid invocation ID/);
  });

  it('rejects an empty agent ID on construction', () => {
    expect(() => new InvocationId('', uuid)).toThrow(/Invalid invocation ID/);
  });
});
