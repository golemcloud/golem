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
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod';
import { schemaValueToWit, v } from '../src/internal/schema-model';
import { selectSelfAgent } from './selfAgentTestSupport';

const input = schemaValueToWit(v.record([]));

const pair = (subject: string) => ({ issuer: 'https://login.example', subject });
const user = (subject: string, claims = '{"role":"reader"}') =>
  ({
    tag: 'user',
    val: {
      pair: pair(subject),
      claims,
      authenticatedAt: { seconds: 1_700_000_000n, nanoseconds: 5 },
    },
  }) as const;
const uuid = (highBits: bigint) => ({ highBits, lowBits: 7n });
const account = () =>
  ({
    tag: 'account',
    val: { pair: pair('account:1'), accountId: { uuid: uuid(1n) }, authentication: 'api-token' },
  }) as const;
const agent = () =>
  ({
    tag: 'agent',
    val: {
      pair: pair('agent:2/3'),
      agentId: { uuid: uuid(2n) },
      agentType: 'Relay',
      environmentId: { uuid: uuid(4n) },
    },
  }) as const;
const tool = () =>
  ({
    tag: 'tool',
    val: { pair: pair('tool:4/report'), environmentId: { uuid: uuid(4n) }, toolName: 'report' },
  }) as const;
const anonymous = { tag: 'anonymous' } as const;

describe('callers read from the invocation context', () => {
  beforeEach(() => vi.resetModules());

  it('gives init the creator and each method invocation its own caller', async () => {
    const sdk = await import('../src');
    const seen: string[] = [];
    sdk
      .defineAgent({
        name: 'CallerProbe',
        id: {},
        methods: {
          who: sdk.method({ input: {}, returns: z.void() }),
        },
      })
      .implement({
        init: (ctx) => {
          seen.push(`init:${ctx.caller.original}`);
          return {};
        },
        methods: {
          who(_input, ctx) {
            seen.push(`method:${ctx.caller.original} via ${ctx.caller.immediate}`);
          },
        },
      });
    selectSelfAgent('CallerProbe');

    const invocationContext = await import('golem:agent/invocation-context@2.0.0');
    const caller = vi.mocked(invocationContext.caller);
    caller.mockReturnValueOnce({ original: user('root'), immediate: account(), tokens: [] });
    await sdk.golemAgent200Guest.initialize('CallerProbe', input);
    caller.mockReturnValueOnce({ original: user('root'), immediate: agent(), tokens: [] });
    await sdk.golemAgent200Guest.invoke('who', input);
    caller.mockReturnValueOnce({ original: anonymous, immediate: tool(), tokens: [] });
    await sdk.golemAgent200Guest.invoke('who', input);

    expect(seen).toEqual([
      'init:user ("https://login.example","root")',
      'method:user ("https://login.example","root") via agent ("https://login.example","agent:2/3")',
      'method:anonymous via tool ("https://login.example","tool:4/report")',
    ]);
  });

  it('reads the host only when the caller is first used', async () => {
    const sdk = await import('../src');
    sdk
      .defineAgent({
        name: 'LazyCallerProbe',
        id: {},
        methods: { ignore: sdk.method({ input: {}, returns: z.void() }) },
      })
      .implement({ init: () => ({}), methods: { ignore() {} } });
    selectSelfAgent('LazyCallerProbe');

    const invocationContext = await import('golem:agent/invocation-context@2.0.0');
    const caller = vi.mocked(invocationContext.caller);
    caller.mockClear();
    await sdk.golemAgent200Guest.initialize('LazyCallerProbe', input);
    await sdk.golemAgent200Guest.invoke('ignore', input);
    expect(caller).not.toHaveBeenCalled();
  });
});

describe('Caller and Principal', () => {
  it('wraps every principal kind and both roles with the token provenance', async () => {
    const { callerFromHost, PrincipalRef } = await import('../src/caller');
    const caller = callerFromHost({
      original: user('u-1', '{"role":"admin","note":"secret"}'),
      immediate: agent(),
      tokens: [
        { id: uuid(9n), maker: pair('account:1') },
        { id: uuid(10n), maker: undefined },
      ],
    });

    expect(caller.original.kind).toBe('user');
    expect(caller.immediate.kind).toBe('agent');
    expect(caller.tokens).toHaveLength(2);
    expect(
      caller.tokens[0].maker?.equals(new PrincipalRef('https://login.example', 'account:1')),
    ).toBe(true);
    expect(caller.tokens[1].maker).toBeUndefined();

    const original = caller.original;
    if (original.kind !== 'user') throw new Error('a user');
    expect(original.issuer).toBe('https://login.example');
    expect(original.subject).toBe('u-1');
    expect(original.claimsJson).toBe('{"role":"admin","note":"secret"}');
    expect(original.claims).toEqual({ role: 'admin', note: 'secret' });
    expect(original.authenticatedAt.getTime()).toBe(1_700_000_000_000);
    expect(original.toString()).toBe('user ("https://login.example","u-1")');
    expect(original.toString()).not.toContain('secret');

    const immediate = caller.immediate;
    if (immediate.kind !== 'agent') throw new Error('an agent');
    expect(immediate.agentType).toBe('Relay');
    expect(immediate.agentId.uuid.highBits).toBe(2n);
    expect(immediate.agentId.uuid.lowBits).toBe(7n);
  });

  it('prints each kind as "<kind> <ref text>" and anonymous as "anonymous"', async () => {
    const { callerFromHost } = await import('../src/caller');
    const caller = callerFromHost({ original: account(), immediate: anonymous, tokens: [] });
    expect(caller.original.toString()).toBe('account ("https://login.example","account:1")');
    expect(caller.immediate.toString()).toBe('anonymous');
    expect(caller.immediate.ref).toBeUndefined();
  });

  it('compares principals by identity and gives anonymous no identity', async () => {
    const { callerFromHost } = await import('../src/caller');
    const first = callerFromHost({ original: user('u-1'), immediate: user('u-1'), tokens: [] });
    const sameIdentity = callerFromHost({
      original: user('u-1', '{"iat":2}'),
      immediate: user('u-1'),
      tokens: [],
    });
    const other = callerFromHost({ original: user('u-2'), immediate: user('u-2'), tokens: [] });
    const nobody = callerFromHost({ original: anonymous, immediate: anonymous, tokens: [] });
    expect(first.original.equals(sameIdentity.original)).toBe(true);
    expect(first.original.equals(other.original)).toBe(false);
    expect(first.original.equals(first.original.ref!)).toBe(true);
    expect(nobody.original.equals(nobody.immediate)).toBe(false);
    expect(first.original.equals(nobody.original)).toBe(false);
  });
});
