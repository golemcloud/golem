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
import { AgentId, InvocationId } from '../src';
import { Uuid } from '../src/uuid';

const agentIdText = '00000007-0000-8000-8000-000000000004';
const agentId = AgentId.parse(agentIdText);
const uuidText = '0123abcd-4567-89ef-fedc-ba9876543210';
const uuid = new Uuid(0x0123abcd456789efn, 0xfedcba9876543210n);

describe('InvocationId', () => {
  it('round-trips the <agent-id>.<uuid> text form', () => {
    const id = new InvocationId(agentId, uuid);
    const text = id.toString();
    expect(text).toBe(`${agentIdText}.${uuidText}`);

    const parsed = InvocationId.parse(text);
    expect(parsed.agentId).toBe(agentId);
    expect(parsed.uuid).toEqual(uuid);
    expect(parsed.toString()).toBe(text);
  });

  it.each([
    ['a missing dot', `${agentIdText}${uuidText}`],
    ['a dot at the wrong position', `${agentIdText}.${uuidText}x`],
    ['a text shorter than two UUIDs', `${agentIdText}.1234`],
    ['a bad UUID', `${agentIdText}.0123abcd-4567-89ef-fedc-ba987654321g`],
    ['an agent ID that is not a UUID', `Agent("a.b").${uuidText}`],
    ['an empty agent ID', `.${uuidText}`],
    ['an empty text', ''],
  ])('rejects %s', (_, text) => {
    expect(() => InvocationId.parse(text)).toThrow(/Invalid invocation ID/);
  });
});

describe('AgentId', () => {
  it('is the lower-case UUID text', () => {
    expect(agentId).toBe(agentIdText);
    expect(JSON.stringify({ agentId })).toBe(`{"agentId":"${agentIdText}"}`);
    expect(AgentId.parse(agentIdText.toUpperCase())).toBe(agentId);
    expect(new Map([[agentId, 1]]).get(AgentId.parse(agentIdText))).toBe(1);
  });

  it('converts to and from a WIT agent-id record', () => {
    const raw = AgentId.toWit(AgentId.parse('0123abcd-4567-89ef-fedc-ba9876543210'));
    expect(raw.uuid.highBits).toBe(0x0123abcd456789efn);
    expect(raw.uuid.lowBits).toBe(0xfedcba9876543210n);
    expect(AgentId.fromWit(raw)).toBe('0123abcd-4567-89ef-fedc-ba9876543210');
  });

  it.each([['Counter("a")'], ['Counter()[0123abcd-4567-89ef-fedc-ba9876543210]'], ['']])(
    'rejects the text %s',
    (text) => {
      expect(() => AgentId.parse(text)).toThrow(/Invalid agent ID/);
    },
  );
});
