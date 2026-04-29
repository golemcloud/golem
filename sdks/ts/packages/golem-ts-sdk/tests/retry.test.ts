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

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  Duration,
  NamedPolicy,
  Policy,
  Predicate,
  Props,
  toRawDuration,
  toRawNamedPolicy,
  toRawPredicateValue,
} from '../src/host/retryBuilder';

describe('retry builder', () => {
  it('compiles predicates and policies into flattened raw WIT shapes', () => {
    const appliesWhen = Predicate.eq(Props.verb, 'get').and(Predicate.gte(Props.statusCode, 500));
    const policy = NamedPolicy.named(
      'http-errors',
      Policy.exponential(Duration.seconds(1), 2)
        .maxRetries(3)
        .withJitter(0.25)
        .onlyWhen(Predicate.matchesGlob(Props.errorType, 'timeout*')),
    )
      .priority(10)
      .appliesWhen(appliesWhen);

    expect(policy.toRaw()).toEqual({
      name: 'http-errors',
      priority: 10,
      predicate: {
        nodes: [
          { tag: 'pred-and', val: [1, 2] },
          {
            tag: 'prop-eq',
            val: {
              propertyName: 'verb',
              value: { tag: 'text', val: 'get' },
            },
          },
          {
            tag: 'prop-gte',
            val: {
              propertyName: 'status-code',
              value: { tag: 'integer', val: 500n },
            },
          },
        ],
      },
      policy: {
        nodes: [
          {
            tag: 'filtered-on',
            val: {
              predicate: {
                nodes: [
                  {
                    tag: 'prop-matches',
                    val: {
                      propertyName: 'error-type',
                      pattern: 'timeout*',
                    },
                  },
                ],
              },
              inner: 1,
            },
          },
          {
            tag: 'jitter',
            val: {
              factor: 0.25,
              inner: 2,
            },
          },
          {
            tag: 'count-box',
            val: {
              maxRetries: 3,
              inner: 3,
            },
          },
          {
            tag: 'exponential',
            val: {
              baseDelay: 1_000_000_000n,
              factor: 2,
            },
          },
        ],
      },
    });
  });

  it('converts duration and property helpers ergonomically', () => {
    expect(Duration.minutes(2).toRaw()).toBe(120_000_000_000n);
    expect(toRawDuration(42)).toBe(42n);
    expect(Props.entries({ [Props.verb]: 'invoke', [Props.statusCode]: 503 })).toEqual([
      ['verb', { tag: 'text', val: 'invoke' }],
      ['status-code', { tag: 'integer', val: 503n }],
    ]);
    expect(toRawPredicateValue(true)).toEqual({ tag: 'boolean', val: true });
  });

  it('validates unsafe integer and policy inputs', () => {
    expect(() => toRawPredicateValue(1.5)).toThrow('safe integer');
    expect(() => toRawPredicateValue(1n << 63n)).toThrow('signed 64-bit');
    expect(() => toRawDuration(-1)).toThrow('non-negative');
    expect(() => toRawDuration(Number.MAX_SAFE_INTEGER + 1)).toThrow('safe integer');
    expect(() => Policy.exponential(Duration.seconds(1), 0).toRaw()).toThrow('greater than 0');
    expect(() => Policy.immediate().withJitter(-0.1).toRaw()).toThrow('greater than or equal to 0');
    expect(() =>
      Policy.immediate().clamp(Duration.seconds(2), Duration.seconds(1)).toRaw(),
    ).toThrow('less than or equal');
    expect(() =>
      NamedPolicy.named('bad-priority', Policy.immediate()).priority(-1).toRaw(),
    ).toThrow('32-bit integer');
  });

  it('passes raw named policies through untouched', () => {
    const raw = {
      name: 'raw',
      priority: 1,
      predicate: { nodes: [{ tag: 'pred-true' as const }] },
      policy: { nodes: [{ tag: 'immediate' as const }] },
    };

    expect(toRawNamedPolicy(raw)).toBe(raw);
  });

  it('rejects spoofed branded objects that do not implement toRaw', () => {
    const spoofedNamedPolicy = {
      [Symbol.for('golem.retry.named-policy')]: true,
    } as unknown as Parameters<typeof toRawNamedPolicy>[0];

    expect(() =>
      toRawDuration({ [Symbol.for('golem.retry.duration')]: true } as unknown as bigint),
    ).toThrow('safe integer');

    expect(toRawNamedPolicy(spoofedNamedPolicy)).toBe(spoofedNamedPolicy);
  });
});

describe('retry host helpers', () => {
  let getRetryPolicyByNameMock: ReturnType<typeof vi.fn>;
  let setRetryPolicyMock: ReturnType<typeof vi.fn>;
  let removeRetryPolicyMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    getRetryPolicyByNameMock = vi.fn();
    setRetryPolicyMock = vi.fn();
    removeRetryPolicyMock = vi.fn();
  });

  afterEach(() => {
    vi.doUnmock('golem:api/retry@1.5.0');
    vi.resetModules();
  });

  async function loadRetryModule() {
    vi.resetModules();
    vi.doMock('golem:api/retry@1.5.0', () => ({
      getRetryPolicies: vi.fn(() => []),
      getRetryPolicyByName: getRetryPolicyByNameMock,
      resolveRetryPolicy: vi.fn(),
      setRetryPolicy: setRetryPolicyMock,
      removeRetryPolicy: removeRetryPolicyMock,
    }));

    return import('../src/host/retry');
  }

  it('setRetryPolicy accepts a NamedPolicy builder', async () => {
    const retry = await loadRetryModule();
    const policy = NamedPolicy.named('builder', Policy.immediate());

    retry.setRetryPolicy(policy);

    expect(setRetryPolicyMock).toHaveBeenCalledWith({
      name: 'builder',
      priority: 0,
      predicate: { nodes: [{ tag: 'pred-true' }] },
      policy: { nodes: [{ tag: 'immediate' }] },
    });
  });

  it('useRetryPolicy restores the previous raw policy on drop', async () => {
    const previous = {
      name: 'builder',
      priority: 1,
      predicate: { nodes: [{ tag: 'pred-true' as const }] },
      policy: { nodes: [{ tag: 'never' as const }] },
    };

    getRetryPolicyByNameMock.mockReturnValue(previous);

    const retry = await loadRetryModule();
    const guard = retry.useRetryPolicy(NamedPolicy.named('builder', Policy.immediate()));

    expect(setRetryPolicyMock).toHaveBeenNthCalledWith(1, {
      name: 'builder',
      priority: 0,
      predicate: { nodes: [{ tag: 'pred-true' }] },
      policy: { nodes: [{ tag: 'immediate' }] },
    });

    guard.drop();

    expect(setRetryPolicyMock).toHaveBeenNthCalledWith(2, previous);
    expect(removeRetryPolicyMock).not.toHaveBeenCalled();
  });

  it('withRetryPolicy removes newly added policies after the callback completes', async () => {
    getRetryPolicyByNameMock.mockReturnValue(undefined);

    const retry = await loadRetryModule();
    const result = retry.withRetryPolicy(NamedPolicy.named('temporary', Policy.immediate()), () => {
      return 'ok';
    });

    expect(result).toBe('ok');
    expect(removeRetryPolicyMock).toHaveBeenCalledWith('temporary');
  });
});
