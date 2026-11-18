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
import { callerFromHost, PrincipalRef } from '../src/caller';

const pair = (subject: string) => ({ issuer: 'https://login.example', subject });
const uuid = (highBits: bigint) => ({ highBits, lowBits: 7n });
const caller = callerFromHost({
  original: {
    tag: 'user',
    val: {
      pair: pair('bob'),
      claims: '{"role":"reader"}',
      authenticatedAt: { seconds: 1_700_000_000n, nanoseconds: 5 },
    },
  },
  immediate: {
    tag: 'agent',
    val: {
      pair: pair('agent:2/3'),
      agentId: { componentId: { uuid: uuid(2n) }, agentId: 'Relay("r")' },
      agentType: 'Relay',
      environmentId: { uuid: uuid(4n) },
    },
  },
  tokens: [{ id: { highBits: 1n, lowBits: 2n }, maker: pair('account:1') }],
});
const others = [
  callerFromHost({
    original: {
      tag: 'account',
      val: { pair: pair('account:1'), accountId: { uuid: uuid(1n) }, authentication: 'api-token' },
    },
    immediate: {
      tag: 'tool',
      val: { pair: pair('tool:4/report'), environmentId: { uuid: uuid(4n) }, toolName: 'report' },
    },
    tokens: [],
  }),
];

describe('ordinary serialization of a live caller', () => {
  it('refuses a caller, each principal kind and a token provenance', () => {
    const live = [
      caller,
      caller.original,
      caller.immediate,
      caller.tokens[0],
      others[0]!.original,
      others[0]!.immediate,
      callerFromHost({ original: { tag: 'anonymous' }, immediate: { tag: 'anonymous' }, tokens: [] })
        .original,
    ];
    for (const value of live) {
      expect(() => JSON.stringify(value)).toThrow(/live value.*ctx\.caller\.original\.ref/);
      expect(() => JSON.stringify({ nested: [value] })).toThrow(/live value/);
    }
  });

  it('keeps a principal reference serializable', () => {
    const ref = caller.original.ref!;
    expect(ref).toBeInstanceOf(PrincipalRef);
    expect(JSON.parse(JSON.stringify(ref))).toEqual({
      issuer: 'https://login.example',
      subject: 'bob',
    });
  });
});
