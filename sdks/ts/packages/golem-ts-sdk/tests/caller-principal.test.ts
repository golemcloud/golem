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

const input = schemaValueToWit(v.record([]));
const oidc = (sub: string) =>
  ({ tag: 'oidc', val: { sub, issuer: 'issuer', claims: '{}' } }) as const;
const subOf = (principal: { tag: string; val?: unknown }) =>
  principal.tag === 'oidc' ? (principal.val as { sub: string }).sub : principal.tag;

describe('principals read from the invocation context', () => {
  beforeEach(() => vi.resetModules());

  it('gives init the creator and each method invocation its own immediate caller', async () => {
    const sdk = await import('../src');
    const { s } = await import('../src/schema/markers');
    const seen: string[] = [];
    sdk
      .defineAgent({
        name: 'CallerProbe',
        id: {},
        methods: {
          who: sdk.method({ input: { caller: s.principal() }, returns: z.void() }),
        },
      })
      .implement({
        init: ({ principal }) => {
          seen.push(`init:${subOf(principal)}`);
          return {};
        },
        methods: {
          who({ caller }) {
            seen.push(`method:${subOf(caller)} agent:${subOf(this.getPrincipal())}`);
          },
        },
      });
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `CallerProbe(${JSON.stringify(input)})`;

    const invocationContext = await import('golem:agent/invocation-context@2.0.0');
    const caller = vi.mocked(invocationContext.caller);
    caller.mockReturnValueOnce({ original: oidc('root'), immediate: oidc('creator') });
    await sdk.golemAgent200Guest.initialize('CallerProbe', input);
    caller.mockReturnValueOnce({ original: oidc('root'), immediate: oidc('first') });
    await sdk.golemAgent200Guest.invoke('who', input);
    caller.mockReturnValueOnce({ original: oidc('root'), immediate: oidc('second') });
    await sdk.golemAgent200Guest.invoke('who', input);

    expect(seen).toEqual([
      'init:creator',
      'method:first agent:creator',
      'method:second agent:creator',
    ]);
  });
});
