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
import { schemaValueFromWit, schemaValueToWit, v } from '../src/internal/schema-model';
import { compileSchema } from '../src/schema/adapter';
import { s as markers } from '../src/schema/markers';
import type { AgentStream } from '../src/schema/agentStream';
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

describe('a context whose call has ended', () => {
  beforeEach(() => vi.resetModules());

  async function setup() {
    const sdk = await import('../src');
    const kept: {
      ctx?: { readonly caller: { original: { toString(): string } } };
      again?: () => string;
    } = {};
    const seen: string[] = [];
    sdk
      .defineAgent({
        name: 'EndedContextProbe',
        id: {},
        methods: {
          read: sdk.method({ input: {}, returns: z.void() }),
          readKept: sdk.method({ input: {}, returns: z.void() }),
          keep: sdk.method({ input: {}, returns: z.void() }),
          capture: sdk.method({ input: {}, returns: z.void() }),
          stream: sdk.method({ input: {}, returns: sdk.s.stream(z.string()) }),
        },
      })
      .implement({
        init: () => ({}),
        methods: {
          read(_input, ctx) {
            seen.push(ctx.caller.original.toString());
          },
          keep(_input, ctx) {
            kept.ctx = ctx;
          },
          capture(_input, ctx) {
            const first = ctx.caller.original.toString();
            kept.again = () => `${first}|${ctx.caller.original.toString()}`;
          },
          readKept() {
            seen.push(kept.ctx!.caller.original.toString());
          },
          stream(_input, ctx) {
            return sdk.AgentStream.from(
              (async function* () {
                yield ctx.caller.original.toString();
              })(),
            );
          },
        },
      });
    selectSelfAgent('EndedContextProbe');
    const invocationContext = await import('golem:agent/invocation-context@2.0.0');
    const caller = vi.mocked(invocationContext.caller);
    await sdk.golemAgent200Guest.initialize('EndedContextProbe', input);
    return { sdk, kept, seen, caller };
  }

  const host = (subject: string) => ({
    original: {
      tag: 'account',
      val: { pair: pair(subject), accountId: { uuid: uuid(1n) }, authentication: 'api-token' },
    } as const,
    immediate: anonymous,
    tokens: [],
  });

  it('gives a context read during its call that call caller', async () => {
    const { sdk, seen, caller } = await setup();
    caller.mockReturnValue(host('account:A'));
    await sdk.golemAgent200Guest.invoke('read', input);
    expect(seen).toEqual(['account ("https://login.example","account:A")']);
  });

  it('refuses a first read after the call ended', async () => {
    const { sdk, kept, seen, caller } = await setup();
    caller.mockReturnValue(host('account:A'));
    await sdk.golemAgent200Guest.invoke('keep', input);
    caller.mockReturnValue(host('account:B'));
    await expect(sdk.golemAgent200Guest.invoke('readKept', input)).rejects.toMatchObject({
      tag: 'custom-error',
    });
    expect(seen).toEqual([]);
    expect(() => kept.ctx!.caller).toThrow(/belongs to a call that has ended/);
  });

  it('keeps the value a context read during its call', async () => {
    const { sdk, kept, caller } = await setup();
    caller.mockReturnValue(host('account:A'));
    await sdk.golemAgent200Guest.invoke('capture', input);
    caller.mockReturnValue(host('account:B'));
    expect(kept.again!()).toBe(
      'account ("https://login.example","account:A")|account ("https://login.example","account:A")',
    );
  });

  it('gives a returned stream the caller of its call when the producer first reads it later', async () => {
    const { sdk, caller } = await setup();
    caller.mockReturnValue(host('account:A'));
    const tree = await sdk.golemAgent200Guest.invoke('stream', input);
    caller.mockReturnValue(host('account:B'));
    const reader = compileSchema(markers.stream(z.string())).fromValue(
      schemaValueFromWit(tree!),
    ) as AgentStream<string>;
    await expect(reader.next()).resolves.toEqual({
      done: false,
      value: 'account ("https://login.example","account:A")',
    });
  });
});
